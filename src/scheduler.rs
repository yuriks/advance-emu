use std::marker::Pinned;
use std::ops::Generator;
use std::ops::GeneratorState;
use std::pin::Pin;

pub struct WaitCycles {
    _cycles: u64,
}

macro_rules! wait_cycles {
    ($num:expr) => {
        yield WaitCycles { _cycles: $num }
    };
}

pub trait Task {
    type Return;

    fn step(self: Pin<&mut Self>) -> GeneratorState<WaitCycles, Self::Return>;
}

struct GeneratorTask<G> {
    generator: G,
    _pin: Pinned,
}

impl<G: Generator<Yield = WaitCycles> + 'static> GeneratorTask<G> {
    /// Wraps a Generator in a Task. `generator.resume()` must've never been called before handing
    /// it to this function.
    ///
    /// **Warning:** This function is actually unsafe if `generator.resume()` has already been
    /// called. It is not marked as so to simplify the syntax for the callers.
    pub fn new(generator: G) -> impl Task<Return = G::Return> {
        GeneratorTask {
            generator,
            _pin: Pinned,
        }
    }
}

impl<G: Generator<Yield = WaitCycles> + 'static> Task for GeneratorTask<G> {
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
                GeneratorState::Complete(x) => break x,
            }
        }
    }};
}

#[cfg(test)]
mod tests {
    use super::*;
    use test::Bencher;

    fn big_task2(delay: u64) -> impl Task<Return = u32> {
        GeneratorTask::new(move || {
            wait_cycles!(delay);
            32
        })
    }

    fn big_task1(delay: u64) -> impl Task<Return = ()> {
        GeneratorTask::new(move || loop {
            wait_cycles!(delay);
            chain_task!(big_task2(delay));
        })
    }

    #[bench]
    fn bench_tasks(b: &mut Bencher) {
        let mut tasks = Vec::new();
        let mut queued_tasks: Vec<usize> = Vec::new();
        for i in 0..16 {
            tasks.push(Box::pinned(big_task1(1 + i / 6)));
        }

        b.iter(|| {
            queued_tasks.extend(0..tasks.len());
            for &task_id in &queued_tasks {
                tasks[task_id].as_mut().step();
            }
            queued_tasks.clear();
        });
    }
}
