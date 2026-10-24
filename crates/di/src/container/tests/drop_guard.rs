//! Drop guards retained by a container: dropped once, after the container's instances.

use std::sync::Mutex;

use super::*;

/// Records its label into a shared log when dropped.
struct DropRecorder {
    label: &'static str,
    log: Arc<Mutex<Vec<&'static str>>>,
}

impl Drop for DropRecorder {
    fn drop(&mut self) {
        self.log
            .lock()
            .expect("drop log is not poisoned")
            .push(self.label);
    }
}

fn recorder(label: &'static str, log: &Arc<Mutex<Vec<&'static str>>>) -> DropRecorder {
    DropRecorder {
        label,
        log: Arc::clone(log),
    }
}

#[tokio::test]
async fn retained_guard_drops_once_after_the_container_instances() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let instance = BoxedComponent {
        ty: TypeDescriptor::of::<DropRecorder>("DropRecorder"),
        value: Box::new(recorder("instance", &log)),
    };
    let root = ScopeContainer::build_root(&[], vec![instance], ResolverSet::new(), registry())
        .await
        .expect("root builds");

    let retained = root.retain_drop_guard(recorder("guard", &log));
    let rejected = root
        .retain_drop_guard(recorder("second guard", &log))
        .expect_err("a container retains only one guard");

    assert!(retained.is_ok());
    assert_eq!(rejected.label, "second guard");

    drop(rejected);
    drop(root);

    assert_eq!(
        *log.lock().expect("drop log is not poisoned"),
        ["second guard", "instance", "guard"]
    );
}
