use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::collections::HashMap;
use std::ops::Generator;
use std::ops::GeneratorState;
use std::pin::Pin;

struct ScheduledTask {
    scheduled_at: u64,
    task_id: usize,
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
        self.scheduled_at == other.scheduled_at && self.task_id == other.task_id
    }
}

impl Eq for ScheduledTask {}

struct Task<T> {
    generator: Pin<Box<Generator<Yield = WaitCycles, Return = T>>>,
}

impl<T> Task<T> {
    // TODO: Make this actually safe if that's even possible. Might not be. Maybe a macro?
    fn new(generator: impl Generator<Yield = WaitCycles, Return = T> + 'static) -> Task<T> {
        Task {
            generator: Box::pinned(generator),
        }
    }

    fn step(&mut self) -> GeneratorState<WaitCycles, T> {
        unsafe { Pin::get_mut_unchecked(self.generator.as_mut()).resume() }
    }
}

struct TaskScheduler {
    current_time: u64,

    scheduled_tasks: BinaryHeap<ScheduledTask>,

    next_task_id: usize,
    active_tasks: HashMap<usize, Task<()>>,
}

impl TaskScheduler {
    fn alloc_task(&mut self) -> usize {
        let task_id = self.next_task_id;
        self.next_task_id += 1;
        task_id
    }

    fn new() -> TaskScheduler {
        TaskScheduler {
            current_time: 0,
            scheduled_tasks: BinaryHeap::new(),
            next_task_id: 0,
            active_tasks: HashMap::new(),
        }
    }

    fn run(&mut self) {
        for _ in 0..30 {
            let now = self.current_time;

            if self.active_tasks.is_empty() {
                assert!(self.scheduled_tasks.is_empty());
                println!("Nothing more to run");
                break;
            }

            println!("[cycle {}]", now);

            loop {
                if let Some(scheduled) = self.scheduled_tasks.peek() {
                    if scheduled.scheduled_at != now {
                        break;
                    }
                } else {
                    break;
                }
                let scheduled = self.scheduled_tasks.pop().unwrap();
                let task_id = scheduled.task_id;
                let result = self.active_tasks.get_mut(&task_id).unwrap().step();
                match result {
                    GeneratorState::Yielded(WaitCycles(cycles)) => {
                        self.scheduled_tasks.push(ScheduledTask {
                            scheduled_at: self.current_time + cycles,
                            task_id,
                        });
                    }
                    GeneratorState::Complete(()) => {
                        self.active_tasks.remove(&task_id);
                    }
                }
            }

            self.current_time += 1;
        }
        println!("Done");
    }

    fn add_task(self: &mut TaskScheduler, task: Task<()>) {
        let task_id = self.alloc_task();
        self.active_tasks.insert(task_id, task);
        self.scheduled_tasks.push(ScheduledTask {
            scheduled_at: self.current_time,
            task_id,
        });
    }
}

fn wait_cycles(cycles: u64) -> WaitCycles {
    WaitCycles(cycles)
}

struct WaitCycles(u64);

macro_rules! chain_task {
    ($subcall:expr) => {{
        let mut sub_task = $subcall;
        loop {
            match sub_task.step() {
                GeneratorState::Yielded(pause) => yield pause,
                GeneratorState::Complete(x) => break x,
            }
        }
    }};
}

fn test_task1() -> Task<()> {
    Task::new(|| {
        println!("Foo");
        yield wait_cycles(4);
        println!("Bar");
        yield wait_cycles(2);
        println!("Spam");
    })
}

fn other_long_fn() -> Task<u32> {
    Task::new(|| {
        println!("hi");
        yield wait_cycles(10);
        println!("bye");
        123
    })
}

fn test_task2() -> Task<()> {
    Task::new(|| {
        yield wait_cycles(3);
        println!("A");
        yield wait_cycles(3);
        println!("B");
        let read_val = chain_task!(other_long_fn());
        println!("C {:?}", read_val);
        yield wait_cycles(3);
        println!("end");
    })
}

pub fn scheduler_test() {
    let mut scheduler = TaskScheduler::new();
    scheduler.add_task(test_task1());
    scheduler.add_task(test_task2());
    scheduler.run();
}
