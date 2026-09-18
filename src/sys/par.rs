//! A minimal work pool.
//!
//! Walking a disk is dominated by waiting on it, so a handful of threads
//! pulling from one shared cursor is most of what parallelism buys here — and
//! it costs no dependency and no scheduler.

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

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

    let pile = Mutex::new(seed);
    // Threads that hold an item and may still hand more back.
    let busy = AtomicUsize::new(0);
    let collected = Mutex::new(Vec::new());

    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                let mut state = init();
                let mut extra = Vec::new();

                loop {
                    let next = {
                        let mut pile = match pile.lock() {
                            Ok(pile) => pile,
                            Err(poisoned) => poisoned.into_inner(),
                        };
                        match pile.pop() {
                            Some(item) => {
                                busy.fetch_add(1, Ordering::AcqRel);
                                Some(item)
                            }
                            None => None,
                        }
                    };

                    let Some(item) = next else {
                        // Nothing to take. Another thread may still be about to
                        // hand work back, so only stop once none of them can.
                        if busy.load(Ordering::Acquire) == 0 {
                            break;
                        }
                        std::thread::yield_now();
                        continue;
                    };

                    work(&mut state, item, &mut extra);

                    if !extra.is_empty() {
                        let mut pile = match pile.lock() {
                            Ok(pile) => pile,
                            Err(poisoned) => poisoned.into_inner(),
                        };
                        pile.append(&mut extra);
                    }
                    busy.fetch_sub(1, Ordering::AcqRel);
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
