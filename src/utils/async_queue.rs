use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tokio::sync::{Mutex, Notify};

#[derive(Clone)]
pub struct AsyncQueue<T> {
    inner: Arc<AsyncQueueInner<T>>,
}

struct AsyncQueueInner<T> {
    items:            Mutex<VecDeque<T>>,
    capacity:         usize,
    push_notify:      Notify,
    pop_notify:       Notify,
    pop_guard_exists: AtomicBool,
}

impl<T> AsyncQueue<T> {
    pub fn new(capacity: usize) -> Self {
        AsyncQueue {
            inner: Arc::new(AsyncQueueInner {
                capacity,
                items: Mutex::new(VecDeque::with_capacity(capacity)),
                push_notify: Notify::new(),
                pop_notify: Notify::new(),
                pop_guard_exists: AtomicBool::new(false),
            }),
        }
    }

    pub async fn push(&self, item: T) {
        loop {
            let mut items = self.inner.items.lock().await;

            if items.len() <= self.inner.capacity {
                items.push_back(item);

                self.inner.push_notify.notify_one();

                return;
            }

            drop(items);

            self.inner.pop_notify.notified();
        }
    }

    pub async fn pop(&self) -> AsyncPopGuard<T> {
        loop {
            let no_other_guards_exist = !self.inner.pop_guard_exists.load(Ordering::SeqCst);
            let queue_is_not_empty = self.inner.items.lock().await.front().is_some();

            if no_other_guards_exist && queue_is_not_empty {
                self.inner.pop_guard_exists.store(true, Ordering::SeqCst);

                return AsyncPopGuard { queue: self };
            }

            // Either could trigger the pop guard to be available
            tokio::select! {
                _ = self.inner.push_notify.notified() => {}
                _ = self.inner.pop_notify.notified() => {}
            }
        }
    }
}

pub struct AsyncPopGuard<'a, T> {
    queue: &'a AsyncQueue<T>,
}

impl<'a, T> AsyncPopGuard<'a, T>
where
    T: Clone,
{
    pub async fn read(&self) -> Option<T> {
        let items = self.queue.inner.items.lock().await;
        items.front().cloned()
    }

    pub async fn commit(self) {
        let mut items = self.queue.inner.items.lock().await;
        self.queue.inner.pop_notify.notify_one();
        items.pop_front();
    }
}

impl<'a, T> Drop for AsyncPopGuard<'a, T> {
    fn drop(&mut self) {
        self.queue
            .inner
            .pop_guard_exists
            .store(false, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use tokio::time::{sleep, timeout, Duration};

    use super::*;

    #[tokio::test]
    async fn pop_on_empty_queue() {
        let queue: AsyncQueue<i32> = AsyncQueue::new(2);

        let pop_guard = timeout(Duration::from_secs_f32(0.5), queue.pop()).await;

        assert!(pop_guard.is_err(), "Pop on empty queue should timeout");
    }

    #[tokio::test]
    async fn read_and_commit_single_item() {
        let queue: AsyncQueue<i32> = AsyncQueue::new(2);

        queue.push(1).await;

        let pop_guard = queue.pop().await;

        queue.push(2).await;

        assert_eq!(pop_guard.read().await, Some(1));

        pop_guard.commit().await;

        let pop_guard = queue.pop().await;

        assert_eq!(pop_guard.read().await, Some(2));
    }

    #[tokio::test]
    async fn drop_without_commit_does_not_remove_item() {
        let queue: AsyncQueue<i32> = AsyncQueue::new(2);

        queue.push(1).await;

        let pop_guard = queue.pop().await;

        queue.push(2).await;

        assert_eq!(pop_guard.read().await, Some(1));

        // Drop without committing
        drop(pop_guard);

        let pop_guard = queue.pop().await;
        assert_eq!(pop_guard.read().await, Some(1));
    }

    #[tokio::test]
    async fn only_a_single_pop_guard_can_exist() {
        let queue: AsyncQueue<i32> = AsyncQueue::new(2);

        queue.push(1).await;

        let first_guard = queue.pop().await;
        assert_eq!(first_guard.read().await, Some(1));

        let second_guard = timeout(Duration::from_secs_f32(0.5), queue.pop()).await;

        assert!(second_guard.is_err(), "Pop on empty queue should timeout");

        drop(first_guard);

        let pop_guard = queue.pop().await;
        assert_eq!(pop_guard.read().await, Some(1));
    }
}
