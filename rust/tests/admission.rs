use atlas::{domain::Error, services::admission::Admission};
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::sync::Notify;

#[tokio::test]
async fn cancelled_callers_keep_slots_until_workers_finish() {
    let admission = Admission::new(8, 1);
    let active = Arc::new(AtomicUsize::new(0));
    let release = Arc::new(Notify::new());
    let mut callers = Vec::new();
    for _ in 0..8 {
        let (gate, count, release) = (admission.clone(), active.clone(), release.clone());
        let permit = gate.acquire().unwrap();
        callers.push(tokio::spawn(async move {
            gate.wait(async move {
                let _permit = permit;
                count.fetch_add(1, Ordering::SeqCst);
                release.notified().await;
                count.fetch_sub(1, Ordering::SeqCst);
                Ok(())
            })
            .await
        }));
    }
    tokio::time::timeout(Duration::from_secs(2), async {
        while active.load(Ordering::SeqCst) != 8 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    for caller in callers {
        caller.abort();
    }
    assert_eq!(admission.acquire().err(), Some(Error::CapacityExceeded));
    assert_eq!(active.load(Ordering::SeqCst), 8);
    release.notify_waiters();
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if let Ok(permit) = admission.acquire() {
                drop(permit);
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let permit = admission.acquire().unwrap();
    let started = std::time::Instant::now();
    assert_eq!(
        admission
            .wait(async move {
                let _permit = permit;
                tokio::time::sleep(Duration::from_millis(1200)).await;
                Ok(())
            })
            .await
            .err(),
        Some(Error::QueryTimeout)
    );
    assert!(started.elapsed() < Duration::from_millis(1150));
    // Seven other slots remain usable while the timed-out worker drains.
    let held: Vec<_> = (0..7).map(|_| admission.acquire().unwrap()).collect();
    assert_eq!(admission.acquire().err(), Some(Error::CapacityExceeded));
    drop(held);
}
