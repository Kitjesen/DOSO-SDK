use doso_sdk::{CloudKind, CloudStreamKind, DosoClient};
use jni::objects::{JObject, JString};
use jni::sys::{jfloatArray, jint, jlong, jstring};
use jni::JNIEnv;
use std::collections::HashMap;
use std::ptr;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};
use tokio::sync::oneshot;

const STALE_AFTER: Duration = Duration::from_secs(3);
static SESSIONS: OnceLock<Mutex<HashMap<i64, Arc<Session>>>> = OnceLock::new();
static NEXT_HANDLE: AtomicI64 = AtomicI64::new(1);

struct FrameState {
    status: String,
    latest: Option<Vec<f32>>,
    received: Option<Instant>,
    identity: Option<(String, u32, u32)>,
    pending_reset: bool,
}

struct Session {
    frame: Mutex<FrameState>,
    cancel: Mutex<Option<oneshot::Sender<()>>>,
}

fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn sessions() -> &'static Mutex<HashMap<i64, Arc<Session>>> {
    SESSIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

impl Session {
    fn new(cancel: oneshot::Sender<()>) -> Self {
        Self {
            frame: Mutex::new(FrameState {
                status: "connecting".into(),
                latest: None,
                received: None,
                identity: None,
                pending_reset: false,
            }),
            cancel: Mutex::new(Some(cancel)),
        }
    }

    fn publish(&self, points: Vec<[f32; 3]>, identity: (String, u32, u32), reset: bool) {
        let mut frame = lock(&self.frame);
        if frame.identity.as_ref() == Some(&identity) {
            return;
        }
        let advanced = frame.identity.is_some();
        let changed_scene = frame
            .identity
            .as_ref()
            .is_some_and(|previous| previous.0 != identity.0 || previous.1 != identity.1);
        if reset || changed_scene {
            frame.pending_reset = true;
            frame.latest = None;
        }
        frame.identity = Some(identity);
        frame.received = Some(Instant::now());
        if !advanced {
            frame.status = "waiting".into();
            return;
        }
        if !reset {
            frame.latest = Some(points.into_iter().flatten().collect());
        }
        frame.status = "live".into();
    }

    fn finish(&self, status: String) {
        let mut frame = lock(&self.frame);
        frame.status = status;
        frame.latest = None;
        frame.received = None;
        frame.pending_reset = false;
    }

    fn status(&self) -> String {
        let frame = lock(&self.frame);
        if matches!(frame.status.as_str(), "live" | "waiting")
            && frame
                .received
                .is_some_and(|received| received.elapsed() > STALE_AFTER)
        {
            "stale".into()
        } else {
            frame.status.clone()
        }
    }

    fn take_latest(&self) -> Option<Vec<f32>> {
        let mut frame = lock(&self.frame);
        if frame.status != "live"
            || frame
                .received
                .is_none_or(|received| received.elapsed() > STALE_AFTER)
        {
            frame.latest = None;
            frame.pending_reset = false;
            return None;
        }
        if frame.pending_reset {
            frame.pending_reset = false;
            return Some(Vec::new());
        }
        frame.latest.take()
    }
}

async fn receive(session: &Session, endpoint: String, kind: CloudKind) -> Result<(), String> {
    let robot = DosoClient::connect(endpoint)
        .await
        .map_err(|error| error.to_string())?;
    let mut stream = robot
        .observations()
        .watch_point_cloud(kind)
        .await
        .map_err(|error| error.to_string())?;
    lock(&session.frame).status = "waiting".into();
    while let Some(message) = stream.message().await.map_err(|error| error.to_string())? {
        let cloud = message.decode().map_err(|error| error.to_string())?;
        session.publish(
            cloud.points,
            (cloud.frame_id, cloud.epoch, cloud.sequence),
            cloud.stream_kind == CloudStreamKind::Reset,
        );
    }
    Ok(())
}

fn start(endpoint: String, kind: CloudKind) -> Result<i64, std::io::Error> {
    let (cancel, mut cancelled) = oneshot::channel();
    let session = Arc::new(Session::new(cancel));
    let handle = NEXT_HANDLE.fetch_add(1, Ordering::Relaxed);
    lock(sessions()).insert(handle, session.clone());
    let task = session.clone();
    let spawned = std::thread::Builder::new()
        .name("doso-point-cloud".into())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    task.finish(format!("error:{error}"));
                    return;
                }
            };
            runtime.block_on(async {
                tokio::select! {
                    biased;
                    _ = &mut cancelled => task.finish("ended".into()),
                    result = receive(&task, endpoint, kind) => {
                        task.finish(match result {
                            Ok(()) => "ended".into(),
                            Err(error) => format!("error:{error}"),
                        });
                    }
                }
            });
        });
    if let Err(error) = spawned {
        lock(sessions()).remove(&handle);
        return Err(error);
    }
    Ok(handle)
}

fn stop(handle: i64) {
    if let Some(session) = lock(sessions()).remove(&handle) {
        session.finish("ended".into());
        if let Some(cancel) = lock(&session.cancel).take() {
            let _ = cancel.send(());
        }
    }
}

#[no_mangle]
pub extern "system" fn Java_expo_modules_dosopointcloud_NativeCloud_start(
    mut env: JNIEnv,
    _owner: JObject,
    endpoint: JString,
    kind: jint,
) -> jlong {
    let kind = match kind {
        1 => CloudKind::Map,
        2 => CloudKind::Scan,
        _ => {
            let _ = env.throw_new(
                "java/lang/IllegalArgumentException",
                "Point-cloud kind must be 1 or 2",
            );
            return 0;
        }
    };
    let endpoint: String = match env.get_string(&endpoint) {
        Ok(endpoint) => endpoint.into(),
        Err(_) => return 0,
    };
    match start(endpoint, kind) {
        Ok(handle) => handle,
        Err(error) => {
            let _ = env.throw_new("java/lang/IllegalStateException", error.to_string());
            0
        }
    }
}

#[no_mangle]
pub extern "system" fn Java_expo_modules_dosopointcloud_NativeCloud_poll(
    env: JNIEnv,
    _owner: JObject,
    handle: jlong,
) -> jfloatArray {
    let session = lock(sessions()).get(&handle).cloned();
    let points = session.and_then(|session| session.take_latest());
    let Some(points) = points else {
        return ptr::null_mut();
    };
    let array = match env.new_float_array(points.len() as jint) {
        Ok(array) => array,
        Err(_) => return ptr::null_mut(),
    };
    if env.set_float_array_region(&array, 0, &points).is_err() {
        return ptr::null_mut();
    }
    array.into_raw()
}

#[no_mangle]
pub extern "system" fn Java_expo_modules_dosopointcloud_NativeCloud_status(
    env: JNIEnv,
    _owner: JObject,
    handle: jlong,
) -> jstring {
    let status = lock(sessions())
        .get(&handle)
        .map_or_else(|| "ended".into(), |session| session.status());
    match env.new_string(status) {
        Ok(status) => status.into_raw(),
        Err(_) => ptr::null_mut(),
    }
}

#[no_mangle]
pub extern "system" fn Java_expo_modules_dosopointcloud_NativeCloud_stop(
    _env: JNIEnv,
    _owner: JObject,
    handle: jlong,
) {
    stop(handle);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latest_frame_replaces_old_and_reset_clears_scene() {
        let (cancel, _receiver) = oneshot::channel();
        let session = Session::new(cancel);
        session.publish(vec![[1.0, 2.0, 3.0]], ("map".into(), 1, 1), false);
        assert_eq!(session.status(), "waiting");
        assert!(lock(&session.frame).latest.is_none());
        session.publish(vec![[4.0, 5.0, 6.0]], ("map".into(), 1, 2), false);
        assert_eq!(session.take_latest(), Some(vec![4.0, 5.0, 6.0]));
        assert!(session.take_latest().is_none());
        session.publish(vec![], ("map".into(), 2, 0), true);
        assert_eq!(session.take_latest(), Some(vec![]));
        assert!(session.take_latest().is_none());
        session.publish(vec![[1.0, 2.0, 3.0]], ("map".into(), 2, 1), false);
        session.finish("error:offline".into());
        assert!(lock(&session.frame).latest.is_none());
        assert_eq!(session.status(), "error:offline");
    }

    #[test]
    fn stalled_frames_are_not_reported_as_live() {
        let (cancel, _receiver) = oneshot::channel();
        let session = Session::new(cancel);
        session.publish(vec![[1.0, 2.0, 3.0]], ("map".into(), 1, 1), false);
        assert_eq!(session.status(), "waiting");
        lock(&session.frame).received = Some(Instant::now() - STALE_AFTER - Duration::from_secs(1));
        assert_eq!(session.status(), "stale");
        assert!(session.take_latest().is_none());
        session.publish(vec![[1.0, 2.0, 3.0]], ("map".into(), 1, 1), false);
        assert_eq!(session.status(), "stale");
        session.publish(vec![[4.0, 5.0, 6.0]], ("map".into(), 1, 2), false);
        assert_eq!(session.status(), "live");
        lock(&session.frame).received = Some(Instant::now() - STALE_AFTER - Duration::from_secs(1));
        assert_eq!(session.status(), "stale");
    }

    #[test]
    fn reset_is_delivered_before_points_that_arrive_before_poll() {
        let (cancel, _receiver) = oneshot::channel();
        let session = Session::new(cancel);
        session.publish(vec![[1.0, 2.0, 3.0]], ("map".into(), 1, 1), false);
        session.publish(vec![[4.0, 5.0, 6.0]], ("map".into(), 1, 2), false);
        assert_eq!(session.take_latest(), Some(vec![4.0, 5.0, 6.0]));

        session.publish(vec![], ("map".into(), 2, 0), true);
        session.publish(vec![[7.0, 8.0, 9.0]], ("map".into(), 2, 1), false);
        session.publish(vec![[10.0, 11.0, 12.0]], ("map".into(), 2, 2), false);
        assert_eq!(session.take_latest(), Some(vec![]));
        assert_eq!(session.take_latest(), Some(vec![10.0, 11.0, 12.0]));
        assert!(session.take_latest().is_none());
    }

    #[test]
    fn coordinate_frame_and_epoch_changes_each_require_refit() {
        for (frame_id, epoch) in [("odom", 1), ("map", 2)] {
            let (cancel, _receiver) = oneshot::channel();
            let session = Session::new(cancel);
            session.publish(vec![[1.0, 2.0, 3.0]], ("map".into(), 1, 1), false);
            session.publish(vec![[4.0, 5.0, 6.0]], ("map".into(), 1, 2), false);
            assert!(session.take_latest().is_some());

            session.publish(vec![[70.0, 80.0, 90.0]], (frame_id.into(), epoch, 3), false);
            session.publish(
                vec![[100.0, 110.0, 120.0]],
                (frame_id.into(), epoch, 4),
                false,
            );
            assert_eq!(session.take_latest(), Some(vec![]));
            assert_eq!(session.take_latest(), Some(vec![100.0, 110.0, 120.0]));
        }
    }

    #[test]
    fn first_reset_stale_and_terminal_states_do_not_replay_points() {
        let (cancel, _receiver) = oneshot::channel();
        let session = Session::new(cancel);
        session.publish(vec![], ("map".into(), 1, 0), true);
        assert_eq!(session.status(), "waiting");
        assert!(session.take_latest().is_none());
        session.publish(vec![[1.0, 2.0, 3.0]], ("map".into(), 1, 1), false);
        assert_eq!(session.take_latest(), Some(vec![1.0, 2.0, 3.0]));

        session.publish(vec![], ("map".into(), 2, 0), true);
        session.publish(vec![[7.0, 8.0, 9.0]], ("map".into(), 2, 1), false);
        lock(&session.frame).received = Some(Instant::now() - STALE_AFTER - Duration::from_secs(1));
        assert_eq!(session.status(), "stale");
        assert!(session.take_latest().is_none());
        assert!(session.take_latest().is_none());

        session.publish(vec![], ("map".into(), 3, 0), true);
        session.publish(vec![[10.0, 11.0, 12.0]], ("map".into(), 3, 1), false);
        session.finish("ended".into());
        assert_eq!(session.status(), "ended");
        assert!(session.take_latest().is_none());
        assert!(!lock(&session.frame).pending_reset);
    }

    #[test]
    fn stop_removes_handle_and_signals_cancellation_once() {
        let (cancel, mut receiver) = oneshot::channel();
        let handle = NEXT_HANDLE.fetch_add(1, Ordering::Relaxed);
        lock(sessions()).insert(handle, Arc::new(Session::new(cancel)));
        stop(handle);
        stop(handle);
        assert_eq!(receiver.try_recv(), Ok(()));
        assert!(!lock(sessions()).contains_key(&handle));
    }

    #[test]
    fn stop_cancels_a_connection_waiting_for_server_response() {
        use std::io::Read;
        use std::net::TcpListener;
        use std::sync::mpsc;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let (accepted, ready) = mpsc::channel();
        let (closed, done) = mpsc::channel();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            accepted.send(()).unwrap();
            let mut buffer = [0_u8; 4096];
            loop {
                match socket.read(&mut buffer) {
                    Ok(0) => {
                        closed.send(()).unwrap();
                        return;
                    }
                    Ok(_) => {}
                    Err(error)
                        if matches!(
                            error.kind(),
                            std::io::ErrorKind::ConnectionReset
                                | std::io::ErrorKind::ConnectionAborted
                        ) =>
                    {
                        closed.send(()).unwrap();
                        return;
                    }
                    Err(error) => panic!("connection was not cancelled: {error}"),
                }
            }
        });
        let handle = start(endpoint, CloudKind::Scan).unwrap();
        ready.recv_timeout(Duration::from_secs(3)).unwrap();
        let before = Instant::now();
        stop(handle);
        assert!(before.elapsed() < Duration::from_secs(1));
        done.recv_timeout(Duration::from_secs(3)).unwrap();
        server.join().unwrap();
    }
}
