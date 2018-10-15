use futures::future::LocalFutureObj;
use std::cell::Cell;
use std::cell::RefCell;
use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::collections::HashMap;
use std::future::Future;
use std::num::NonZeroUsize;
use std::pin::Pin;
use std::sync::Arc;
use std::task;
use std::task::LocalWaker;
use std::task::Poll;
use std::task::Wake;

type TaskId = NonZeroUsize;

thread_local! {
    static TASK_SCHEDULER: TaskScheduler = TaskScheduler::new();
    static CURRENT_TASK: Cell<Option<TaskId>> = Cell::new(None);
}

struct DummyWake;

impl Wake for DummyWake {
    fn wake(_arc_self: &Arc<Self>) {
        // ???
    }
}

struct ScheduledTask {
    scheduled_at: u64,
    scheduling_id: usize, // Used for sort tie-breaking
    task_id: TaskId,
}

impl Ord for ScheduledTask {
    fn cmp(&self, other: &Self) -> Ordering {
        (other.scheduled_at, other.scheduling_id).cmp(&(self.scheduled_at, self.scheduling_id))
    }
}

impl PartialOrd for ScheduledTask {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for ScheduledTask {
    fn eq(&self, other: &Self) -> bool {
        self.scheduled_at == other.scheduled_at && self.scheduling_id == other.scheduling_id
    }
}

impl Eq for ScheduledTask {}

struct TaskSchedulerInner {
    current_time: u64,

    scheduled_tasks: BinaryHeap<ScheduledTask>,

    scheduling_counter: usize,
    next_task_id: usize,

    wait_queue: HashMap<TaskId, LocalFutureObj<'static, ()>>,
}

impl TaskSchedulerInner {
    fn next_scheduling_id(&mut self) -> usize {
        let next_id = self.scheduling_counter;
        self.scheduling_counter += 1;
        next_id
    }

    fn reschedule_task(&mut self, at: u64, task_id: TaskId) {
        let scheduling_id = self.next_scheduling_id();
        self.scheduled_tasks.push(ScheduledTask {
            scheduled_at: at,
            scheduling_id,
            task_id,
        });
    }

    fn alloc_task(&mut self) -> TaskId {
        let task_id = NonZeroUsize::new(self.next_task_id).unwrap();
        self.next_task_id += 1;

        task_id
    }
}

struct TaskScheduler {
    inner: RefCell<TaskSchedulerInner>,
}

impl TaskScheduler {
    fn new() -> TaskScheduler {
        TaskScheduler {
            inner: RefCell::new(TaskSchedulerInner {
                current_time: 0,
                scheduled_tasks: BinaryHeap::new(),
                scheduling_counter: 0,
                next_task_id: 1,
                wait_queue: HashMap::new(),
            }),
        }
    }

    fn wait_cycles(self: &TaskScheduler, cycles: u64) -> WaitCycles {
        let inner = self.inner.borrow();
        let scheduled_time = inner.current_time + cycles;

        WaitCycles {
            at: scheduled_time,
            task_id: CURRENT_TASK.with(|current_task| current_task.get().unwrap()),
        }
    }

    fn run(self: &TaskScheduler) {
        for _ in 0..30 {
            let mut inner = self.inner.borrow_mut();
            let now = inner.current_time;

            if inner.scheduled_tasks.is_empty() && inner.wait_queue.is_empty() {
                println!("Nothing more to run");
                break;
            }

            println!("[cycle {}]", now);

            loop {
                if let Some(task) = inner.scheduled_tasks.peek() {
                    if task.scheduled_at != now {
                        break;
                    }
                } else {
                    break;
                }
                let scheduled_task = inner.scheduled_tasks.pop().unwrap();
                let task_id = scheduled_task.task_id;
                let mut task = inner.wait_queue.remove(&task_id).unwrap();

                drop(inner);
                let poll_result;
                {
                    let pinned_task = Pin::new(&mut task);
                    let waker = task::local_waker_from_nonlocal(Arc::new(DummyWake));
                    CURRENT_TASK.with(|current_task| current_task.set(Some(task_id)));
                    poll_result = pinned_task.poll(&waker);
                    CURRENT_TASK.with(|current_task| current_task.set(None));
                };
                inner = self.inner.borrow_mut();
                if poll_result == Poll::Pending {
                    inner.wait_queue.insert(task_id, task);
                } else {
                    println!("Finished task {}", task_id);
                }
            }

            inner.current_time += 1;
        }
        println!("Done");
    }

    fn add_task(self: &TaskScheduler, f: impl Future<Output = ()> + 'static) {
        let mut inner = self.inner.borrow_mut();

        let task_id = inner.alloc_task();
        let task = LocalFutureObj::new(Box::new(f));

        let now = inner.current_time;
        let scheduling_id = inner.next_scheduling_id();
        inner.wait_queue.insert(task_id, task);
        inner.scheduled_tasks.push(ScheduledTask {
            scheduled_at: now,
            scheduling_id,
            task_id,
        });
    }
}

fn wait_cycles(cycles: u64) -> WaitCycles {
    TASK_SCHEDULER.with(|scheduler| scheduler.wait_cycles(cycles))
}

struct WaitCycles {
    at: u64,
    task_id: TaskId,
}

impl Future for WaitCycles {
    type Output = ();

    fn poll(self: Pin<&mut Self>, _: &LocalWaker) -> Poll<()> {
        TASK_SCHEDULER.with(|scheduler| {
            let mut scheduler = scheduler.inner.borrow_mut();

            assert!(scheduler.current_time <= self.at);
            if scheduler.current_time == self.at {
                Poll::Ready(())
            } else {
                scheduler.reschedule_task(self.at, self.task_id);
                Poll::Pending
            }
        })
    }
}

async fn test_task1() {
    println!("Foo");
    await!(wait_cycles(4));
    println!("Bar");
    await!(wait_cycles(2));
    println!("Spam");
}

async fn other_long_fn() -> u32 {
    println!("hi");
    await!(wait_cycles(10));
    println!("bye");
    123
}

async fn test_task2() {
    await!(wait_cycles(3));
    println!("A");
    await!(wait_cycles(3));
    println!("B");
    let read_val = await!(other_long_fn());
    println!("C: {}", read_val);
    await!(wait_cycles(3));
    println!("end");
}

pub fn scheduler_test() {
    TASK_SCHEDULER.with(|scheduler| {
        scheduler.add_task(test_task1());
        scheduler.add_task(test_task2());
        scheduler.run();
    });
}
