use futures::future::LocalFutureObj;
use std::cell::RefCell;
use std::cmp;
use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::collections::HashMap;
use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::Arc;
use std::task;
use std::task::LocalWaker;
use std::task::Poll;
use std::task::Wake;

thread_local! {
    static TASK_SCHEDULER: Rc<TaskScheduler> = TaskScheduler::new();
}

struct WakeupEvent {
    task_id: usize,
    waker: LocalWaker,
}

struct WakeToken(usize);

impl Wake for WakeToken {
    fn wake(arc_self: &Arc<Self>) {
        let WakeToken(task_id) = **arc_self;

        TASK_SCHEDULER.with(|scheduler: &Rc<TaskScheduler>| {
            let event = WakeupEvent {
                task_id,
                waker: unsafe { task::local_waker(arc_self.clone()) },
            };
            scheduler.run_queue.borrow_mut().push_back(event);
        });
    }
}

struct ScheduledTask {
    scheduled_at: u64,
    scheduling_id: usize, // Used for sort tie-breaking
    waker: LocalWaker,
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

    wait_queue: HashMap<usize, LocalFutureObj<'static, ()>>,
}

impl TaskSchedulerInner {
    fn next_scheduling_id(&mut self) -> usize {
        let next_id = self.scheduling_counter;
        self.scheduling_counter += 1;
        next_id
    }

    fn reschedule_task(&mut self, at: u64, lw: LocalWaker) {
        let scheduling_id = self.next_scheduling_id();
        self.scheduled_tasks.push(ScheduledTask {
            scheduled_at: at,
            scheduling_id,
            waker: lw,
        });
    }

    fn alloc_task(&mut self) -> (usize, LocalWaker) {
        let task_id = self.next_task_id;
        self.next_task_id += 1;

        (task_id, unsafe {
            task::local_waker(Arc::new(WakeToken(task_id)))
        })
    }
}

struct TaskScheduler {
    inner: RefCell<TaskSchedulerInner>,

    run_queue: RefCell<VecDeque<WakeupEvent>>,
}

impl TaskScheduler {
    fn new() -> Rc<TaskScheduler> {
        Rc::new(TaskScheduler {
            inner: RefCell::new(TaskSchedulerInner {
                current_time: 0,
                scheduled_tasks: BinaryHeap::new(),
                scheduling_counter: 0,
                next_task_id: 0,
                wait_queue: HashMap::new(),
            }),
            run_queue: RefCell::new(VecDeque::new()),
        })
    }

    fn wait_cycles(self: &Rc<TaskScheduler>, cycles: u64) -> WaitCycles {
        let inner = self.inner.borrow();
        let scheduled_time = inner.current_time + cycles;

        WaitCycles { at: scheduled_time }
    }

    fn run(self: &Rc<TaskScheduler>) {
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
                let task = inner.scheduled_tasks.pop().unwrap();
                task.waker.wake();
            }

            let mut run_queue = self.run_queue.borrow_mut();
            while let Some(wakeup_event) = run_queue.pop_front() {
                let mut task = inner.wait_queue.remove(&wakeup_event.task_id).unwrap();
                drop(inner);
                let poll_result = {
                    let pinned_task = Pin::new(&mut task);
                    pinned_task.poll(&wakeup_event.waker)
                };
                inner = self.inner.borrow_mut();
                if poll_result == Poll::Pending {
                    inner.wait_queue.insert(wakeup_event.task_id, task);
                } else {
                    println!("Finished task {}", wakeup_event.task_id);
                }
            }

            inner.current_time += 1;
        }
        println!("Done");
    }

    fn add_task(self: &Rc<TaskScheduler>, f: impl Future<Output = ()> + 'static) {
        let mut inner = self.inner.borrow_mut();

        let (task_id, waker) = inner.alloc_task();
        let task = LocalFutureObj::new(Box::new(f));

        let now = inner.current_time;
        let scheduling_id = inner.next_scheduling_id();
        inner.wait_queue.insert(task_id, task);
        inner.scheduled_tasks.push(ScheduledTask {
            scheduled_at: now,
            scheduling_id,
            waker,
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

    fn poll(self: Pin<&mut Self>, lw: &LocalWaker) -> Poll<()> {
        TASK_SCHEDULER.with(|scheduler| {
            let mut scheduler = scheduler.inner.borrow_mut();

            assert!(scheduler.current_time <= self.at);
            if scheduler.current_time == self.at {
                Poll::Ready(())
            } else {
                scheduler.reschedule_task(self.at, lw.clone());
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

async fn test_task2() {
    await!(wait_cycles(3));
    println!("A");
    await!(wait_cycles(3));
    println!("B");
    await!(wait_cycles(3));
    println!("C");
    await!(wait_cycles(3));
    println!("end");
}

pub fn scheduler_test() {
    TASK_SCHEDULER.with(|scheduler| {
        scheduler.add_task(wait_cycles(15));
        scheduler.add_task(test_task1());
        scheduler.add_task(test_task2());
        scheduler.run();
    });
}
