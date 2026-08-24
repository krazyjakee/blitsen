//! Registered Windows toast COM activation (#252).
//!
//! An in-process toast event handler dies with the process that registered it.
//! Windows' stopped-application path is an out-of-process COM class associated
//! with the toast's AppUserModelID. The registry points that class at the
//! packaged executable; once Windows starts it, this class factory receives the
//! toast's encoded launch argument and commits it to the ordinary replay store.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::Mutex;
use windows::Win32::Foundation::{CLASS_E_NOAGGREGATION, RPC_E_CHANGED_MODE};
use windows::Win32::System::Com::{
    CLSCTX_LOCAL_SERVER, COINIT_APARTMENTTHREADED, CoInitializeEx, CoRegisterClassObject,
    CoRevokeClassObject, IClassFactory, IClassFactory_Impl, REGCLS_MULTIPLEUSE,
};
use windows::Win32::UI::Notifications::{
    INotificationActivationCallback, INotificationActivationCallback_Impl,
    NOTIFICATION_USER_INPUT_DATA,
};
use windows::core::{GUID, IUnknown, Interface, PCWSTR, Ref, implement};
use winit::event_loop::EventLoopProxy;
use winreg::{RegKey, enums::HKEY_CURRENT_USER};

use super::{ActivationStore, activator_clsid, activator_uuid, decode_desktop_envelope};

const COM_SERVER_ARGUMENT: &str = "--notification-com-server";

/// Registers the AppUserModelID-to-COM mapping for the executable at its final
/// installed location. The packaging companion `.reg` file writes the same
/// mapping before first launch; this refresh makes moving a portable build
/// recover on its next ordinary launch instead of retaining a stale path.
pub(super) fn register(identity: &str, display_name: &str) -> Result<(), String> {
    let executable = std::env::current_exe()
        .map_err(|error| format!("could not locate the Windows notification activator: {error}"))?;
    register_at(identity, display_name, &executable)
}

fn register_at(identity: &str, display_name: &str, executable: &Path) -> Result<(), String> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let classes = hkcu
        .create_subkey(format!(r"SOFTWARE\Classes\AppUserModelId\{identity}"))
        .map_err(|error| format!("could not register Windows notification identity: {error}"))?
        .0;
    let clsid = activator_clsid(identity);
    classes
        .set_value("DisplayName", &display_name)
        .and_then(|()| classes.set_value("CustomActivator", &clsid))
        .map_err(|error| format!("could not register Windows notification activator: {error}"))?;

    let server = hkcu
        .create_subkey(format!(r"SOFTWARE\Classes\CLSID\{clsid}\LocalServer32"))
        .map_err(|error| format!("could not register Windows notification COM server: {error}"))?
        .0;
    let command = format!("\"{}\" {COM_SERVER_ARGUMENT}", executable.display());
    server
        .set_value("", &command)
        .map_err(|error| format!("could not register Windows notification COM server: {error}"))
}

#[implement(INotificationActivationCallback)]
struct ToastActivator {
    directory: PathBuf,
    application_identity: String,
    errors: Arc<Mutex<VecDeque<(String, String)>>>,
    proxy: EventLoopProxy,
}

impl INotificationActivationCallback_Impl for ToastActivator_Impl {
    fn Activate(
        &self,
        app_user_model_id: &PCWSTR,
        invoked_args: &PCWSTR,
        _data: *const NOTIFICATION_USER_INPUT_DATA,
        _count: u32,
    ) -> windows::core::Result<()> {
        // SAFETY: COM owns both NUL-terminated strings for the duration of this
        // callback, which is exactly the lifetime used by `to_string`.
        let addressed_to = unsafe { app_user_model_id.to_string() }?;
        if addressed_to != self.application_identity {
            return Ok(());
        }
        let argument = unsafe { invoked_args.to_string() }?;
        let activation = match decode_desktop_envelope(&argument) {
            Ok(activation)
                if activation.identity == self.application_identity
                    && activation.entry == self.application_identity =>
            {
                activation
            }
            Ok(_) => return Ok(()),
            Err(error) => {
                self.errors.lock().push_back((String::new(), error));
                self.proxy.wake_up();
                return Ok(());
            }
        };
        let id = activation.id.clone();
        if let Err(error) =
            ActivationStore::new(&self.directory, &self.application_identity).record(activation)
        {
            self.errors.lock().push_back((id, error));
        }
        self.proxy.wake_up();
        Ok(())
    }
}

#[implement(IClassFactory)]
struct ActivationFactory {
    directory: PathBuf,
    application_identity: String,
    errors: Arc<Mutex<VecDeque<(String, String)>>>,
    proxy: EventLoopProxy,
}

impl IClassFactory_Impl for ActivationFactory_Impl {
    fn CreateInstance(
        &self,
        outer: Ref<'_, IUnknown>,
        iid: *const GUID,
        object: *mut *mut core::ffi::c_void,
    ) -> windows::core::Result<()> {
        if !outer.is_null() {
            return Err(windows::core::Error::from_hresult(CLASS_E_NOAGGREGATION));
        }
        let callback: INotificationActivationCallback = ToastActivator {
            directory: self.directory.clone(),
            application_identity: self.application_identity.clone(),
            errors: Arc::clone(&self.errors),
            proxy: self.proxy.clone(),
        }
        .into();
        // SAFETY: COM supplied a writable output slot and interface IID to the
        // class factory. `query` writes an AddRef'd pointer on success.
        unsafe { callback.query(iid, object).ok() }
    }

    fn LockServer(&self, _lock: windows::core::BOOL) -> windows::core::Result<()> {
        Ok(())
    }
}

/// Keeps the registered class factory alive for the window session.
pub(super) struct ComServer {
    cookie: u32,
}

impl ComServer {
    pub(super) fn start(
        directory: PathBuf,
        identity: String,
        errors: Arc<Mutex<VecDeque<(String, String)>>>,
        proxy: EventLoopProxy,
    ) -> Result<Self, String> {
        // Winit normally initialises the UI thread's apartment first. S_OK and
        // S_FALSE both allow class registration; RPC_E_CHANGED_MODE means a
        // compatible apartment already exists and is likewise not fatal.
        let initialized = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        if initialized.is_err() && initialized != RPC_E_CHANGED_MODE {
            return Err(format!(
                "could not initialise the Windows notification COM server: {initialized:?}"
            ));
        }
        let class = GUID::from_u128(activator_uuid(&identity));
        let factory: IClassFactory = ActivationFactory {
            directory,
            application_identity: identity,
            errors,
            proxy,
        }
        .into();
        // SAFETY: `factory` is a valid class factory and COM retains it for the
        // registration represented by the returned cookie.
        let cookie = unsafe {
            CoRegisterClassObject(&class, &factory, CLSCTX_LOCAL_SERVER, REGCLS_MULTIPLEUSE)
        }
        .map_err(|error| {
            format!("could not register the Windows notification COM class: {error}")
        })?;
        Ok(Self { cookie })
    }
}

impl Drop for ComServer {
    fn drop(&mut self) {
        // SAFETY: this cookie was returned by `CoRegisterClassObject` on the
        // same session thread and is revoked at most once here.
        let _ = unsafe { CoRevokeClassObject(self.cookie) };
    }
}
