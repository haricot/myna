//! Capture-driven completion must leave the next shortcut as a fresh Press.
use async_trait::async_trait;
use myna_desktop::{indicator::mock::MockIndicator, inject::mock::MockInjector};
use myna_desktop::{DesktopController, Live, SessionRun};
use myna_orchestrator::{OrchestratorEvent, SessionOutcome, StopHandle, Trigger, TriggerEdge};
use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::sync::mpsc;

struct Toggles {
    rx: mpsc::Receiver<()>,
    pressed: bool,
    resyncs: Arc<AtomicUsize>,
}
#[async_trait]
impl Trigger for Toggles {
    async fn next_edge(&mut self) -> Option<TriggerEdge> {
        self.rx.recv().await?;
        self.pressed = !self.pressed;
        Some(if self.pressed {
            TriggerEdge::Press
        } else {
            TriggerEdge::Release
        })
    }
    async fn resync(&mut self) {
        self.pressed = false;
        self.resyncs.fetch_add(1, Ordering::SeqCst);
    }
}
async fn wait_for(mut f: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(3), async {
        while !f() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the controller reaches its terminal state");
}
#[tokio::test]
async fn confirmed_capture_end_resets_toggle_parity_and_preserves_preview_commit_order() {
    let (poke, rx) = mpsc::channel(4);
    let resyncs = Arc::new(AtomicUsize::new(0));
    let trigger = Toggles {
        rx,
        pressed: false,
        resyncs: resyncs.clone(),
    };
    let injector = MockInjector::new().with_preedit_support();
    let log = injector.log();
    let session = move |events: mpsc::Sender<OrchestratorEvent>| {
        let stop = StopHandle::default();
        let confirmed = stop.clone();
        let run: SessionRun = Box::pin(async move {
            events.send(OrchestratorEvent::Ready).await.unwrap();
            events
                .send(OrchestratorEvent::Unstable("Bonj".into()))
                .await
                .unwrap();
            // The backend stops this capture, without a shortcut Release.
            confirmed.stop();
            events
                .send(OrchestratorEvent::Final("Bonjour".into()))
                .await
                .unwrap();
            events
                .send(OrchestratorEvent::Done("Bonjour".into()))
                .await
                .unwrap();
            Ok(SessionOutcome::Completed {
                transcript: "Bonjour".into(),
            })
        });
        (run, stop)
    };
    let mut controller = DesktopController::builder()
        .trigger(trigger)
        .injector(injector)
        .indicator(MockIndicator::new())
        .preedit(Live::new(true))
        .session(session)
        .build();
    let task = tokio::spawn(async move {
        controller.run().await;
    });
    poke.send(()).await.unwrap();
    wait_for(|| resyncs.load(Ordering::SeqCst) >= 1).await;
    poke.send(()).await.unwrap();
    wait_for(|| resyncs.load(Ordering::SeqCst) >= 2).await;
    drop(poke);
    tokio::time::timeout(Duration::from_secs(3), task)
        .await
        .unwrap()
        .unwrap();
    let log = log.lock().unwrap();
    assert_eq!(log.acquires, 2);
    assert_eq!(log.releases, 2);
    assert_eq!(
        log.commits.iter().map(|s| s.trim()).collect::<Vec<_>>(),
        vec!["Bonjour", "Bonjour"]
    );
    assert_eq!(log.preedits, vec!["Bonj", "Bonj"]);
}
