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

fn postupdate_cell<T: Copy>(cell: &Cell<T>, f: fn(T) -> T) -> T {
    let t = cell.get();
    cell.set(f(t));
    t
}

struct DummyWake;

impl Wake for DummyWake {
    fn wake(_arc_self: &Arc<Self>) {
        // ???
    }
}

struct ScheduledTask {
    scheduled_at: u64,
    task_id: TaskId,
}

impl Ord for ScheduledTask {
    fn cmp(&self, other: &Self) -> Ordering {
        (other.scheduled_at, other.task_id).cmp(&(self.scheduled_at, self.task_id))
    }
}

impl PartialOrd for ScheduledTask {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for ScheduledTask {
    fn eq(&self, other: &Self) -> bool {
        (self.scheduled_at, self.task_id) == (other.scheduled_at, other.task_id)
    }
}

impl Eq for ScheduledTask {}

struct TaskScheduler {
    current_time: Cell<u64>,

    scheduled_tasks: RefCell<BinaryHeap<ScheduledTask>>,

    next_task_id: Cell<usize>,
    active_tasks: RefCell<HashMap<TaskId, LocalFutureObj<'static, ()>>>,
}

impl TaskScheduler {
    fn new() -> TaskScheduler {
        TaskScheduler {
            current_time: 0.into(),
            scheduled_tasks: BinaryHeap::new().into(),
            next_task_id: 1.into(),
            active_tasks: HashMap::new().into(),
        }
    }

    fn reschedule_task(&self, at: u64, task_id: TaskId) {
        self.scheduled_tasks.borrow_mut().push(ScheduledTask {
            scheduled_at: at,
            task_id,
        });
    }

    fn alloc_task(&self) -> TaskId {
        let task_id = postupdate_cell(&self.next_task_id, |x| x + 1);
        NonZeroUsize::new(task_id).unwrap()
    }

    fn wait_cycles(self: &TaskScheduler, cycles: u64) -> WaitCycles {
        WaitCycles { at: self.current_time.get() + cycles }
    }

    fn run(self: &TaskScheduler) {
        for _ in 0..30 {
            let mut scheduled_tasks = self.scheduled_tasks.borrow_mut();
            let mut active_tasks = self.active_tasks.borrow_mut();

            assert_eq!(scheduled_tasks.is_empty(), active_tasks.is_empty());
            if active_tasks.is_empty() {
                println!("Nothing more to run");
                break;
            }

            let now = self.current_time.get();
            println!("[cycle {}]", now);

            loop {
                if let Some(task) = scheduled_tasks.peek() {
                    if task.scheduled_at != now {
                        break;
                    }
                } else {
                    break;
                }
                let scheduled_task = scheduled_tasks.pop().unwrap();
                let task_id = scheduled_task.task_id;

                // scheduled_tasks might be modified by poll()
                drop(scheduled_tasks);
                let poll_result;
                {
                    let mut task = active_tasks.get_mut(&task_id).unwrap();
                    let waker = task::local_waker_from_nonlocal(Arc::new(DummyWake));
                    CURRENT_TASK.with(|current_task| current_task.set(Some(task_id)));
                    poll_result = Pin::new(&mut task).poll(&waker);
                    CURRENT_TASK.with(|current_task| current_task.set(None));
                };
                scheduled_tasks = self.scheduled_tasks.borrow_mut();

                match poll_result {
                    Poll::Ready(()) => {
                        active_tasks.remove(&task_id);
                        println!("Finished task {}", task_id)
                    },
                    Poll::Pending => {}
                }
            }

            postupdate_cell(&self.current_time, |x| x + 1);
        }
        println!("Done");
    }

    fn add_task(self: &TaskScheduler, f: impl Future<Output = ()> + 'static) {
        let task_id = self.alloc_task();
        let task = LocalFutureObj::new(Box::new(f));

        let now = self.current_time.get();
        self.active_tasks.borrow_mut().insert(task_id, task);
        self.scheduled_tasks.borrow_mut().push(ScheduledTask {
            scheduled_at: now,
            task_id,
        });
    }
}

fn wait_cycles(cycles: u64) -> WaitCycles {
    TASK_SCHEDULER.with(|scheduler| scheduler.wait_cycles(cycles))
}

struct WaitCycles {
    at: u64,
}

impl Future for WaitCycles {
    type Output = ();

    fn poll(self: Pin<&mut Self>, _: &LocalWaker) -> Poll<()> {
        TASK_SCHEDULER.with(|scheduler: &TaskScheduler| {
            let current_time = scheduler.current_time.get();
            assert!(current_time <= self.at);
            if current_time == self.at {
                Poll::Ready(())
            } else {
                let task_id = CURRENT_TASK
                    .with(|current_task| current_task.get().expect("No currently active task"));
                scheduler.reschedule_task(self.at, task_id);
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
        scheduler.add_task(wait_cycles(10));
        scheduler.add_task(test_task1());
        scheduler.add_task(test_task2());
        scheduler.run();
    });
}
