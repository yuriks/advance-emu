use std::cmp::Ord;
use std::cmp::Ordering;
use std::collections::binary_heap::PeekMut;
use std::collections::BinaryHeap;
use std::marker::PhantomPinned;
use std::ops::Generator;
use std::ops::GeneratorState;
use std::pin::Pin;

pub struct WaitCycles {
    cycles: u64,
}

#[inline(always)]
pub fn wait_cycles(cycles: u64) -> WaitCycles {
    WaitCycles { cycles }
}

macro_rules! wait_cycles {
    ($num:expr) => {
        yield ::scheduler::wait_cycles($num)
    };
}

pub trait Task<'ctx> {
    type Return;

    fn step(self: Pin<&mut Self>) -> GeneratorState<WaitCycles, Self::Return>;
}

pub struct GeneratorTask<G> {
    generator: G,
    _pin: PhantomPinned,
}

impl<G: Generator<Yield = WaitCycles>> GeneratorTask<G> {
    /// Wraps a Generator in a Task. `generator.resume()` must've never been called before handing
    /// it to this function.
    ///
    /// **Warning:** This function is actually unsafe if `generator.resume()` has already been
    /// called. It is not marked as so to simplify the syntax for the callers.
    pub fn new(generator: G) -> GeneratorTask<G> {
        GeneratorTask {
            generator,
            _pin: PhantomPinned,
        }
    }
}

impl<'ctx, G: Generator<Yield = WaitCycles> + 'ctx> Task<'ctx> for GeneratorTask<G> {
    type Return = G::Return;

    fn step(self: Pin<&mut Self>) -> GeneratorState<WaitCycles, G::Return> {
        // This is safe because Task is !Unpin
        unsafe { Pin::get_mut_unchecked(self).generator.resume() }
    }
}

macro_rules! chain_task {
    ($subcall:expr) => {{
        let mut sub_task = $subcall; // This variable must not be moved
        'l: loop {
            let state = {
                let mut pinned = unsafe { Pin::new_unchecked(&mut sub_task) };
                pinned.step()
            };
            match state {
                GeneratorState::Yielded(pause) => yield pause,
                GeneratorState::Complete(x) => break 'l x,
            }
        }
    }};
}

#[derive(PartialEq, Eq)]
struct ScheduledTask {
    scheduled_at: u64,
    task_id: usize,
}

impl Ord for ScheduledTask {
    fn cmp(&self, other: &Self) -> Ordering {
        // The ordering is reversed because it is used in BinaryHeap which is a max-heap, while we
        // want to pop the smallest element instead.
        other
            .scheduled_at
            .cmp(&self.scheduled_at)
            .then(other.task_id.cmp(&self.task_id))
    }
}

impl PartialOrd for ScheduledTask {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

pub struct TaskScheduler<'g> {
    current_time: u64,

    // TODO: Optimize this using a fixed-size ring buffer for events in the near future, to get fast
    // O(1) push for those instead of using the heap.
    scheduled_tasks: BinaryHeap<ScheduledTask>,

    active_tasks: Vec<Option<Pin<Box<dyn Task<'g, Return = ()>>>>>,
}

impl<'g> TaskScheduler<'g> {
    pub fn new() -> TaskScheduler<'g> {
        TaskScheduler {
            current_time: 0,
            scheduled_tasks: BinaryHeap::new(),
            active_tasks: Vec::new(),
        }
    }

    pub fn current_time(&self) -> u64 {
        self.current_time
    }

    pub fn add_new_task(&mut self, task: Pin<Box<dyn Task<'g, Return = ()>>>) {
        let task_id = self.active_tasks.len();
        self.active_tasks.push(Some(task));
        self.scheduled_tasks.push(ScheduledTask {
            scheduled_at: self.current_time,
            task_id,
        });
    }

    pub fn run_for(&mut self, cycles: u64) {
        if cycles == 0 {
            return;
        }
        let stop_time = self.current_time + cycles;

        'l: loop {
            let mut next_task = match self.scheduled_tasks.peek_mut() {
                Some(task) => task,
                None => break 'l,
            };

            if next_task.scheduled_at >= stop_time {
                break 'l;
            }

            let task_id = next_task.task_id;
            let result = {
                let task = self
                    .active_tasks
                    .get_mut(task_id)
                    .and_then(|x| x.as_mut())
                    .unwrap();
                task.as_mut().step()
            };
            match result {
                GeneratorState::Yielded(WaitCycles { cycles }) => {
                    next_task.scheduled_at += cycles;
                }
                GeneratorState::Complete(()) => {
                    PeekMut::pop(next_task);
                    self.active_tasks.remove(task_id);
                }
            }
        }

        self.current_time = stop_time;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test::Bencher;

    fn big_task2(delay: u64) -> impl Task<'static, Return = u32> {
        GeneratorTask::new(move || {
            wait_cycles!(delay);
            32
        })
    }

    fn big_task1(delay: u64) -> impl Task<'static, Return = ()> {
        GeneratorTask::new(move || loop {
            wait_cycles!(delay);
            chain_task!(big_task2(delay));
        })
    }

    #[bench]
    fn bench_task_switch_overhead(b: &mut Bencher) {
        // Measures speed of cycling between 16 tasks, without any scheduler overhead
        let mut tasks = Vec::new();
        let mut queued_tasks: Vec<usize> = Vec::new();
        for i in 0..16 {
            tasks.push(Box::pinned(big_task1(1 + i / 6)));
        }

        b.iter(|| {
            // Add some basic book-keeping overhead
            queued_tasks.extend(0..tasks.len());
            for &task_id in &queued_tasks {
                tasks[task_id].as_mut().step();
            }
            queued_tasks.clear();
        });
    }

    #[bench]
    fn bench_task_switch_iterate(b: &mut Bencher) {
        // Measures speed of cycling between 16 tasks, without any scheduler overhead
        let mut tasks = Vec::new();
        for i in 0..16 {
            tasks.push(Box::pinned(big_task1(1 + i / 6)));
        }

        b.iter(|| {
            for task in tasks.iter_mut() {
                task.as_mut().step();
            }
        });
    }

    #[bench]
    fn bench_task_scheduler(b: &mut Bencher) {
        // Measures speed of cycling between 16 tasks, using the scheduler
        let mut scheduler = TaskScheduler::new();
        for i in 0..16 {
            scheduler.add_new_task(Box::pinned(big_task1(1 + i / 6)));
        }

        b.iter(|| {
            scheduler.run_for(1);
        });
    }
}
