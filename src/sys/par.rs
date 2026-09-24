//! A minimal work pool.
//!
//! Walking a disk is dominated by waiting on it, so a handful of threads
//! pulling from one shared cursor is most of what parallelism buys here — and
//! it costs no dependency and no scheduler.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex};

/// How many threads to use. Bounded: past a point, more threads only make the
/// disk seek more.
pub fn threads() -> usize {
    std::thread::available_parallelism()
        .map(|count| count.get().clamp(1, 8))
        .unwrap_or(1)
}

/// Applies `work` to every item, on several threads, keeping the input order.
///
/// `progress` is called once per finished item, from whichever thread finished
/// it, with the number done so far.
pub fn map<T, R>(
    items: &[T],
    work: impl Fn(&T) -> R + Sync,
    progress: impl Fn(usize, &T) + Sync,
) -> Vec<R>
where
    T: Sync,
    R: Send,
{
    let workers = threads().min(items.len());
    if workers <= 1 {
        return items
            .iter()
            .enumerate()
            .map(|(index, item)| {
                let result = work(item);
                progress(index + 1, item);
                result
            })
            .collect();
    }

    let cursor = AtomicUsize::new(0);
    let done = AtomicUsize::new(0);
    let results: Mutex<Vec<Option<R>>> = Mutex::new((0..items.len()).map(|_| None).collect());

    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    let index = cursor.fetch_add(1, Ordering::Relaxed);
                    let Some(item) = items.get(index) else { break };

                    let result = work(item);
                    let finished = done.fetch_add(1, Ordering::Relaxed) + 1;
                    progress(finished, item);

                    if let Ok(mut slots) = results.lock() {
                        slots[index] = Some(result);
                    }
                }
            });
        }
    });

    results
        .into_inner()
        .unwrap_or_default()
        .into_iter()
        .flatten()
        .collect()
}

/// Runs `work` over a pile of items that grows as it is consumed.
///
/// This is the shape a directory tree actually has: a worker opens a
/// directory, deals with the files in it, and hands the subdirectories back to
/// the pile for whichever thread is free. Splitting only the top level leaves
/// most threads idle whenever one branch is much bigger than the others, which
/// is every home directory ever.
///
/// Each thread keeps its own state, built by `init`; all of them are returned.
pub fn drain<T, S>(
    seed: Vec<T>,
    init: impl Fn() -> S + Sync,
    work: impl Fn(&mut S, T, &mut Vec<T>) + Sync,
) -> Vec<S>
where
    T: Send,
    S: Send,
{
    if seed.is_empty() {
        return Vec::new();
    }

    let workers = threads();
    if workers <= 1 {
        let mut state = init();
        let mut pile = seed;
        let mut extra = Vec::new();
        while let Some(item) = pile.pop() {
            work(&mut state, item, &mut extra);
            pile.append(&mut extra);
        }
        return vec![state];
    }

    // The pile and the count of threads still working, under one lock: a
    // thread that finds the pile empty sleeps until either changes, instead
    // of spinning — spinning eight threads on `sched_yield` was most of the
    // kernel time of a home-directory walk.
    struct Shared<T> {
        pile: Vec<T>,
        busy: usize,
    }
    let shared = Mutex::new(Shared {
        pile: seed,
        busy: 0,
    });
    let changed = Condvar::new();
    let collected = Mutex::new(Vec::new());
    let lock = || match shared.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };

    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                let mut state = init();
                let mut extra = Vec::new();

                loop {
                    let item = {
                        let mut guard = lock();
                        loop {
                            if let Some(item) = guard.pile.pop() {
                                guard.busy += 1;
                                break Some(item);
                            }
                            if guard.busy == 0 {
                                break None;
                            }
                            guard = match changed.wait(guard) {
                                Ok(guard) => guard,
                                Err(poisoned) => poisoned.into_inner(),
                            };
                        }
                    };
                    let Some(item) = item else {
                        changed.notify_all();
                        break;
                    };

                    work(&mut state, item, &mut extra);

                    let mut guard = lock();
                    let handed_back = extra.len();
                    guard.pile.append(&mut extra);
                    guard.busy -= 1;
                    let finished = guard.busy == 0 && guard.pile.is_empty();
                    drop(guard);
                    if finished {
                        changed.notify_all();
                    } else if handed_back > 0 {
                        for _ in 0..handed_back.min(workers) {
                            changed.notify_one();
                        }
                    }
                }

                if let Ok(mut all) = collected.lock() {
                    all.push(state);
                }
            });
        }
    });

    collected.into_inner().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_the_input_order() {
        let items: Vec<usize> = (0..100).collect();
        let doubled = map(&items, |n| n * 2, |_, _| {});
        assert_eq!(doubled, items.iter().map(|n| n * 2).collect::<Vec<_>>());
    }

    #[test]
    fn counts_every_item_once() {
        let items: Vec<usize> = (0..64).collect();
        let seen = AtomicUsize::new(0);
        map(
            &items,
            |_| (),
            |_, _| {
                seen.fetch_add(1, Ordering::Relaxed);
            },
        );
        assert_eq!(seen.load(Ordering::Relaxed), 64);
    }

    #[test]
    fn an_empty_list_spawns_nothing() {
        let items: Vec<usize> = Vec::new();
        assert!(map(&items, |n| *n, |_, _| {}).is_empty());
        assert!(drain(Vec::<usize>::new(), || 0usize, |_, _, _| {}).is_empty());
    }

    #[test]
    fn drain_sees_every_item_a_growing_pile_produces() {
        // A tree: each number under 50 hands back two more.
        let counts = drain(
            vec![0usize],
            || 0usize,
            |seen: &mut usize, item, extra| {
                *seen += 1;
                if item < 50 {
                    extra.push(item * 2 + 1);
                    extra.push(item * 2 + 2);
                }
            },
        );
        // 51 nodes hand back children, and every node is visited once.
        let total: usize = counts.iter().sum();
        assert_eq!(total, 101);
    }
}
