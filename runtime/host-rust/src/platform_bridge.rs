//! Small Android framework adapter. No charging sysfs writes and no Flutter UI
//! work on Android's main thread. NEESCHAL's rule: report state, don't invent it.
use jni::{JNIEnv, objects::JClass, sys::{jboolean, jint, jfloat}};
use std::sync::{Mutex, OnceLock, atomic::{AtomicI32, Ordering}};

static ACTIVITY: OnceLock<Mutex<Option<(jni::JavaVM, jni::objects::GlobalRef)>>> = OnceLock::new();
static BACK_EVENT: AtomicI32 = AtomicI32::new(0);

#[unsafe(no_mangle)]
pub extern "C" fn rodin_host_consume_bypass_settings_request() -> i32 {
    0
}
// Registration and invocation share the lock: Dart can safely unregister before
// closing its callback, including when Android delivers a final lifecycle event.
static BACK_LISTENER: Mutex<Option<extern "C" fn(i32)>> = Mutex::new(None);

#[unsafe(no_mangle)]
pub extern "C" fn rodin_host_set_back_listener(listener: Option<extern "C" fn(i32)>) {
    *BACK_LISTENER.lock().unwrap() = listener;
    BACK_EVENT.store(0, Ordering::Release);
    super::RODIN_BACK_PENDING.store(false, Ordering::Release);
}

pub fn init(activity: *mut super::ANativeActivity) {
    let Some((vm, object, _)) = super::rodin_photo_vm_and_object(activity) else { return; };
    let reference = {
        let Ok(env) = vm.attach_current_thread() else { return; };
        let object = unsafe { jni::objects::JObject::from_raw(object) };
        env.new_global_ref(&object).ok()
    };
    if let Some(reference) = reference {
        *ACTIVITY.get_or_init(|| Mutex::new(None)).lock().unwrap() = Some((vm, reference));
    }
}
pub fn clear() {
    if let Some(activity) = ACTIVITY.get() { *activity.lock().unwrap() = None; }
    BACK_EVENT.store(0, Ordering::Release);
}
pub fn refresh_back() {
    let Some(activity) = ACTIVITY.get() else { return; };
    let guard = activity.lock().unwrap();
    let Some((vm, reference)) = guard.as_ref() else { return; };
    let Ok(mut env) = vm.attach_current_thread() else { return; };
    let _ = env.with_local_frame(4, |env| -> jni::errors::Result<()> {
        let class = env.get_object_class(reference.as_obj())?;
        env.call_static_method(class, "refreshBackFromNative", "()V", &[])?;
        Ok(())
    });
    if env.exception_check().unwrap_or(false) { let _ = env.exception_clear(); }
}
#[unsafe(no_mangle)]
pub extern "C" fn rodin_host_consume_back_gesture() -> i32 { BACK_EVENT.swap(0, Ordering::AcqRel) }
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_neeschal_rodinessential_RodinActivity_nativeInterceptBack(_: JNIEnv, _: JClass) -> jboolean {
    super::RODIN_BACK_INTERCEPT.load(Ordering::Acquire) as jboolean
}
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_neeschal_rodinessential_RodinActivity_nativeBack(_: JNIEnv, _: JClass, phase: jint, progress: jfloat, edge: jint) {
    if !(1..=4).contains(&phase) || !progress.is_finite() { return; }
    if phase != 2 { super::log_str(&format!("PLATFORM_BACK phase={phase} progress={progress} edge={edge}")); }
    let value = (progress.clamp(0.0, 1.0) * 10000.0).round() as i32;
    // Single atomic packet: phase, edge and progress cannot come from different frames.
    let packet = (phase << 16) | ((edge & 1) << 15) | value;
    let listener = BACK_LISTENER.lock().unwrap();
    if let Some(callback) = *listener {
        callback(packet);
        return;
    }
    BACK_EVENT.store(packet, Ordering::Release);
    if phase == 4 && super::RODIN_BACK_INTERCEPT.load(Ordering::Acquire) {
        super::RODIN_BACK_PENDING.store(true, Ordering::Release);
    }
}

