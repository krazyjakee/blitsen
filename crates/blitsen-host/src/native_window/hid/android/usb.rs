//! `UsbManager` and `UsbDeviceConnection` over JNI, and nothing else.
//!
//! Every decision this file could have made is made in its parent: what counts
//! as a HID interface, what a boot protocol means, when a dialog has been
//! answered, how a report is framed. What is left here is the part that can
//! only be a Java call — and, because a Java call is the one thing this
//! workspace cannot execute (see #151 and the module header), the part that is
//! kept as small and as literal as it can be.
//!
//! It reaches the JVM through the same Activity handle and the same attachment
//! helper `blitsen/notify` established, rather than through a second copy of
//! them: `android-activity` owns the Activity, and there is one of it.

use jni::objects::{JObject, JString, JValue};
use jni::{Env, jni_sig, jni_str};

use crate::native_window::notify::{android_app, with_activity};

use super::{UsbApi, UsbConnection, UsbInterface};

/// The action of the broadcast `requestPermission` sends when it is answered.
///
/// Nothing receives it — the APK's scoped notification bridge declares no USB
/// receiver — and the parent module explains why the answer is read from
/// `hasPermission` instead. It is still named after this package rather than
/// left generic, because an implicit intent with a borrowable action is a way
/// for another application to be woken by this one's permission dialog.
const PERMISSION_ACTION: &str = "dev.blitsen.hid.USB_PERMISSION";

/// `PendingIntent.FLAG_IMMUTABLE`.
///
/// The system fills `EXTRA_DEVICE` and `EXTRA_PERMISSION_GRANTED` into a
/// mutable one, and nothing here reads them. An immutable `PendingIntent`
/// cannot be filled in by whoever ends up holding it, which is the safer of two
/// answers to a question this code does not need asked.
const FLAG_IMMUTABLE: i32 = 1 << 26;

/// `UsbConstants.USB_ENDPOINT_XFER_INT` and `USB_DIR_IN`.
const ENDPOINT_INTERRUPT: i32 = 3;
const DIRECTION_IN: i32 = 0x80;

/// The control-transfer request types a HID class request uses.
///
/// `0x21` is host-to-device, class, interface; `0xa1` is the same the other
/// way; `0x81` is device-to-host, standard, interface, which is what fetches a
/// descriptor. `0x09` is `SET_REPORT`, `0x01` is `GET_REPORT`, `0x06` is
/// `GET_DESCRIPTOR`, and `0x22` is the report descriptor's type.
const CLASS_OUT: i32 = 0x21;
const CLASS_IN: i32 = 0xa1;
const STANDARD_IN: i32 = 0x81;
const SET_REPORT: i32 = 0x09;
const GET_REPORT: i32 = 0x01;
const GET_DESCRIPTOR: i32 = 0x06;
const REPORT_DESCRIPTOR_TYPE: i32 = 0x22;

/// How long a control transfer may take. Milliseconds.
///
/// The same second `hidapi`'s libusb backend gives one. A control transfer is
/// answered by the device's firmware in microseconds when it is answered at
/// all, so this is a ceiling on a failure rather than a budget for a success.
const CONTROL_TIMEOUT: i32 = 1000;

/// The largest report descriptor the HID specification allows.
const DESCRIPTOR_MAX: usize = 4096;

/// The `UsbManager` system service.
fn usb_manager<'env>(
    env: &mut Env<'env>,
    activity: &JObject<'_>,
) -> jni::errors::Result<JObject<'env>> {
    let service = env.new_string("usb")?;
    env.call_method(
        activity,
        jni_str!("getSystemService"),
        jni_sig!("(Ljava/lang/String;)Ljava/lang/Object;"),
        &[JValue::Object(&service)],
    )?
    .l()
}

/// A `String` returned by a getter that answers `null` when it has nothing.
fn optional_string(env: &mut Env<'_>, object: JObject<'_>) -> jni::errors::Result<Option<String>> {
    if object.is_null() {
        return Ok(None);
    }
    let text = env.cast_local::<JString>(object)?;
    let chars = text.mutf8_chars(env)?;
    Ok(Some(chars.to_str().into_owned()))
}

/// Calls a no-argument `String` getter on a device.
fn string_getter(
    env: &mut Env<'_>,
    device: &JObject<'_>,
    name: &jni::strings::JNIStr,
) -> jni::errors::Result<Option<String>> {
    let value = env
        .call_method(device, name, jni_sig!("()Ljava/lang/String;"), &[])?
        .l()?;
    optional_string(env, value)
}

/// Calls a no-argument `int` getter.
fn int_getter(
    env: &mut Env<'_>,
    object: &JObject<'_>,
    name: &jni::strings::JNIStr,
) -> jni::errors::Result<i32> {
    env.call_method(object, name, jni_sig!("()I"), &[])?.i()
}

/// The `UsbDevice` a device name identifies, or a null object if it is gone.
fn device_by_name<'env>(
    env: &mut Env<'env>,
    manager: &JObject<'_>,
    device_name: &str,
) -> jni::errors::Result<JObject<'env>> {
    let list = env
        .call_method(
            manager,
            jni_str!("getDeviceList"),
            jni_sig!("()Ljava/util/HashMap;"),
            &[],
        )?
        .l()?;
    let key = env.new_string(device_name)?;
    env.call_method(
        &list,
        jni_str!("get"),
        jni_sig!("(Ljava/lang/Object;)Ljava/lang/Object;"),
        &[JValue::Object(&key)],
    )?
    .l()
}

/// Reads one `UsbDevice` into the interfaces the parent module works in.
fn interfaces_of(
    env: &mut Env<'_>,
    device: &JObject<'_>,
    permitted: bool,
) -> jni::errors::Result<Vec<UsbInterface>> {
    let device_name = string_getter(env, device, jni_str!("getDeviceName"))?.unwrap_or_default();
    let vendor_id = int_getter(env, device, jni_str!("getVendorId"))?;
    let product_id = int_getter(env, device, jni_str!("getProductId"))?;
    let version = string_getter(env, device, jni_str!("getVersion"))?;
    let manufacturer_name = string_getter(env, device, jni_str!("getManufacturerName"))?;
    let product_name = string_getter(env, device, jni_str!("getProductName"))?;
    // Only with the grant in hand. From API 29 `getSerialNumber` throws
    // `SecurityException` without it, and an enumeration that threw would take
    // the whole device list with it.
    let serial_number = if permitted {
        string_getter(env, device, jni_str!("getSerialNumber"))?
    } else {
        None
    };
    let count = int_getter(env, device, jni_str!("getInterfaceCount"))?;
    let mut interfaces = Vec::new();
    for index in 0..count {
        let interface = env
            .call_method(
                device,
                jni_str!("getInterface"),
                jni_sig!("(I)Landroid/hardware/usb/UsbInterface;"),
                &[JValue::Int(index)],
            )?
            .l()?;
        interfaces.push(UsbInterface {
            device_name: device_name.clone(),
            interface_id: int_getter(env, &interface, jni_str!("getId"))?,
            vendor_id: vendor_id as u16,
            product_id: product_id as u16,
            version: version.clone(),
            interface_class: int_getter(env, &interface, jni_str!("getInterfaceClass"))?,
            interface_subclass: int_getter(env, &interface, jni_str!("getInterfaceSubclass"))?,
            interface_protocol: int_getter(env, &interface, jni_str!("getInterfaceProtocol"))?,
            product_name: product_name.clone(),
            manufacturer_name: manufacturer_name.clone(),
            serial_number: serial_number.clone(),
        });
    }
    Ok(interfaces)
}

/// The interrupt endpoints of a claimed interface, IN first.
fn endpoints<'env>(
    env: &mut Env<'env>,
    interface: &JObject<'_>,
) -> jni::errors::Result<(Option<JObject<'env>>, Option<JObject<'env>>)> {
    let count = int_getter(env, interface, jni_str!("getEndpointCount"))?;
    let (mut input, mut output) = (None, None);
    for index in 0..count {
        let endpoint = env
            .call_method(
                interface,
                jni_str!("getEndpoint"),
                jni_sig!("(I)Landroid/hardware/usb/UsbEndpoint;"),
                &[JValue::Int(index)],
            )?
            .l()?;
        if int_getter(env, &endpoint, jni_str!("getType"))? != ENDPOINT_INTERRUPT {
            continue;
        }
        let inbound = int_getter(env, &endpoint, jni_str!("getDirection"))? == DIRECTION_IN;
        // The first of each direction, which is the one the HID specification
        // gives an interface: a second interrupt IN endpoint on one HID
        // interface is not a thing the class definition describes.
        if inbound && input.is_none() {
            input = Some(endpoint);
        } else if !inbound && output.is_none() {
            output = Some(endpoint);
        }
    }
    Ok((input, output))
}

/// `UsbManager`, reached through the Activity that owns it.
pub(crate) struct ActivityUsb;

impl UsbApi for ActivityUsb {
    type Connection = InterfaceConnection;

    fn interfaces(&mut self) -> Result<Vec<UsbInterface>, String> {
        let app = android_app()?;
        with_activity(&app, |env, activity| {
            let manager = usb_manager(env, activity)?;
            let list = env
                .call_method(
                    &manager,
                    jni_str!("getDeviceList"),
                    jni_sig!("()Ljava/util/HashMap;"),
                    &[],
                )?
                .l()?;
            let values = env
                .call_method(
                    &list,
                    jni_str!("values"),
                    jni_sig!("()Ljava/util/Collection;"),
                    &[],
                )?
                .l()?;
            let iterator = env
                .call_method(
                    &values,
                    jni_str!("iterator"),
                    jni_sig!("()Ljava/util/Iterator;"),
                    &[],
                )?
                .l()?;
            let mut found = Vec::new();
            while env
                .call_method(&iterator, jni_str!("hasNext"), jni_sig!("()Z"), &[])?
                .z()?
            {
                let device = env
                    .call_method(
                        &iterator,
                        jni_str!("next"),
                        jni_sig!("()Ljava/lang/Object;"),
                        &[],
                    )?
                    .l()?;
                let permitted = env
                    .call_method(
                        &manager,
                        jni_str!("hasPermission"),
                        jni_sig!("(Landroid/hardware/usb/UsbDevice;)Z"),
                        &[JValue::Object(&device)],
                    )?
                    .z()?;
                found.extend(interfaces_of(env, &device, permitted)?);
            }
            Ok(found)
        })
    }

    fn has_permission(&mut self, device_name: &str) -> Result<bool, String> {
        let app = android_app()?;
        with_activity(&app, |env, activity| {
            let manager = usb_manager(env, activity)?;
            let device = device_by_name(env, &manager, device_name)?;
            if device.is_null() {
                // A device that is gone holds no grant, which is also what
                // Android does with the grant itself when it is unplugged.
                return Ok(false);
            }
            env.call_method(
                &manager,
                jni_str!("hasPermission"),
                jni_sig!("(Landroid/hardware/usb/UsbDevice;)Z"),
                &[JValue::Object(&device)],
            )?
            .z()
        })
    }

    fn request_permission(&mut self, device_name: &str) -> Result<(), String> {
        let app = android_app()?;
        with_activity(&app, |env, activity| {
            let manager = usb_manager(env, activity)?;
            let device = device_by_name(env, &manager, device_name)?;
            if device.is_null() {
                // Detached between the enumeration and this call. Nothing to
                // ask about, and the open that asked will be told the device is
                // gone on its next turn.
                return Ok(());
            }
            let action = env.new_string(PERMISSION_ACTION)?;
            let intent = env.new_object(
                jni_str!("android/content/Intent"),
                jni_sig!("(Ljava/lang/String;)V"),
                &[JValue::Object(&action)],
            )?;
            // Explicit, so the broadcast cannot be delivered outside this
            // package — required from Android 14 and correct before it.
            let package = env
                .call_method(
                    activity,
                    jni_str!("getPackageName"),
                    jni_sig!("()Ljava/lang/String;"),
                    &[],
                )?
                .l()?;
            let intent = env
                .call_method(
                    &intent,
                    jni_str!("setPackage"),
                    jni_sig!("(Ljava/lang/String;)Landroid/content/Intent;"),
                    &[JValue::Object(&package)],
                )?
                .l()?;
            let pending = env
                .call_static_method(
                    jni_str!("android/app/PendingIntent"),
                    jni_str!("getBroadcast"),
                    jni_sig!(
                        "(Landroid/content/Context;ILandroid/content/Intent;I)\
                         Landroid/app/PendingIntent;"
                    ),
                    &[
                        JValue::Object(activity),
                        JValue::Int(0),
                        JValue::Object(&intent),
                        JValue::Int(FLAG_IMMUTABLE),
                    ],
                )?
                .l()?;
            // Not hopped onto the Java main thread, unlike the notification
            // permission beside it: that one is `Activity.requestPermissions`,
            // which the framework documents as a main-thread call. This is a
            // binder call to the USB service, and the dialog it raises is the
            // service's own activity rather than a callback into ours.
            env.call_method(
                &manager,
                jni_str!("requestPermission"),
                jni_sig!("(Landroid/hardware/usb/UsbDevice;Landroid/app/PendingIntent;)V"),
                &[JValue::Object(&device), JValue::Object(&pending)],
            )?;
            Ok(())
        })
    }

    fn focused(&mut self) -> Result<bool, String> {
        let app = android_app()?;
        with_activity(&app, |env, activity| {
            env.call_method(activity, jni_str!("hasWindowFocus"), jni_sig!("()Z"), &[])?
                .z()
        })
    }

    /// Opens and claims, distinguishing the ways Android can decline.
    ///
    /// The inner `Result` is the platform declining and the outer one is JNI
    /// failing, which are different things: the first is a sentence an
    /// application is shown, the second is a call that did not happen. Both end
    /// up as an `OperationError` — permission was settled before this was
    /// called, so nothing here can be a permission problem — but only the first
    /// can say anything useful about the device.
    fn open(&mut self, interface: &UsbInterface) -> Result<InterfaceConnection, String> {
        let app = android_app()?;
        with_activity(&app, |env, activity| {
            let manager = usb_manager(env, activity)?;
            let device = device_by_name(env, &manager, &interface.device_name)?;
            if device.is_null() {
                return Ok(Err("the device is no longer attached".to_owned()));
            }
            let count = int_getter(env, &device, jni_str!("getInterfaceCount"))?;
            let mut claimed = None;
            for index in 0..count {
                let candidate = env
                    .call_method(
                        &device,
                        jni_str!("getInterface"),
                        jni_sig!("(I)Landroid/hardware/usb/UsbInterface;"),
                        &[JValue::Int(index)],
                    )?
                    .l()?;
                if int_getter(env, &candidate, jni_str!("getId"))? == interface.interface_id {
                    claimed = Some(candidate);
                    break;
                }
            }
            let Some(target) = claimed else {
                return Ok(Err(format!(
                    "the device no longer has interface {}",
                    interface.interface_id
                )));
            };
            let (input, output) = endpoints(env, &target)?;
            let Some(input) = input else {
                // No interrupt IN endpoint is no input reports, which is not a
                // HID device this module can offer anything for.
                return Ok(Err(
                    "the interface declares no interrupt IN endpoint to read reports from"
                        .to_owned(),
                ));
            };
            let connection = env
                .call_method(
                    &manager,
                    jni_str!("openDevice"),
                    jni_sig!("(Landroid/hardware/usb/UsbDevice;)Landroid/hardware/usb/UsbDeviceConnection;"),
                    &[JValue::Object(&device)],
                )?
                .l()?;
            if connection.is_null() {
                return Ok(Err("the USB service refused to open the device".to_owned()));
            }
            // Forced, which detaches whatever kernel driver had the interface.
            // That is the whole point of opening it — an unforced claim of a
            // HID interface fails against the kernel's own HID driver — and it
            // is why the collection filter runs before this and the descriptor
            // check runs immediately after: an interface this module must not
            // have is released again before a report is ever read from it.
            let taken = env
                .call_method(
                    &connection,
                    jni_str!("claimInterface"),
                    jni_sig!("(Landroid/hardware/usb/UsbInterface;Z)Z"),
                    &[JValue::Object(&target), JValue::Bool(true)],
                )?
                .z()?;
            if !taken {
                // Closed here rather than left to a drop that will not happen:
                // nothing owns this connection yet, so this is the only place
                // that can give the device back.
                env.call_method(&connection, jni_str!("close"), jni_sig!("()V"), &[])?;
                return Ok(Err(format!(
                    "interface {} could not be claimed",
                    interface.interface_id
                )));
            }
            Ok(Ok(InterfaceConnection {
                device_name: interface.device_name.clone(),
                interface_number: interface.interface_id,
                connection: env.new_global_ref(&connection)?,
                interface: env.new_global_ref(&target)?,
                endpoint_in: env.new_global_ref(&input)?,
                endpoint_out: output
                    .map(|endpoint| env.new_global_ref(&endpoint))
                    .transpose()?,
            }))
        })?
    }
}

/// One claimed interface, owned by the worker thread that reads it.
///
/// The references are global because they outlive the JNI frame that made them
/// and are used from a thread that did not: a local reference is neither. The
/// strings and integers beside them are what the transfers need and what a
/// teardown needs, so nothing has to go back to `UsbManager` to close.
pub(crate) struct InterfaceConnection {
    device_name: String,
    interface_number: i32,
    connection: jni::objects::Global<JObject<'static>>,
    interface: jni::objects::Global<JObject<'static>>,
    endpoint_in: jni::objects::Global<JObject<'static>>,
    endpoint_out: Option<jni::objects::Global<JObject<'static>>>,
}

impl InterfaceConnection {
    /// One control transfer, in either direction, with the bytes it moved.
    ///
    /// A negative answer is a failure and is reported as one; Android does not
    /// say which, and the parent module never asks it to, because a control
    /// transfer that failed is a device that is not answering rather than a
    /// permission question — that was settled before this handle existed.
    fn control(
        &self,
        request_type: i32,
        request: i32,
        value: i32,
        data: &mut [u8],
        write: bool,
    ) -> Result<usize, String> {
        let app = android_app()?;
        with_activity(&app, |env, _| {
            let array = if write {
                let signed = data.iter().map(|byte| *byte as i8).collect::<Vec<_>>();
                let array = env.new_byte_array(signed.len())?;
                array.set_region(env, 0, &signed)?;
                array
            } else {
                env.new_byte_array(data.len())?
            };
            let moved = env
                .call_method(
                    self.connection.as_obj(),
                    jni_str!("controlTransfer"),
                    jni_sig!("(IIII[BII)I"),
                    &[
                        JValue::Int(request_type),
                        JValue::Int(request),
                        JValue::Int(value),
                        JValue::Int(self.interface_number),
                        JValue::Object(array.as_ref()),
                        JValue::Int(data.len() as i32),
                        JValue::Int(CONTROL_TIMEOUT),
                    ],
                )?
                .i()?;
            if moved < 0 {
                return Ok(moved);
            }
            let moved = (moved as usize).min(data.len());
            if !write {
                let mut signed = vec![0i8; moved];
                array.get_region(env, 0, &mut signed)?;
                for (slot, byte) in data.iter_mut().zip(signed) {
                    *slot = byte as u8;
                }
            }
            Ok(moved as i32)
        })
        .and_then(|moved| {
            usize::try_from(moved).map_err(|_| {
                // Android reports every reason a control transfer did not
                // happen as one negative number, so this cannot say which. It
                // is still worth separating from a JNI failure: the call was
                // made and the device did not answer it.
                format!("the USB control transfer was refused by the device ({moved})")
            })
        })
    }
}

impl UsbConnection for InterfaceConnection {
    fn report_descriptor(&self) -> Result<Vec<u8>, String> {
        let mut buffer = vec![0u8; DESCRIPTOR_MAX];
        let read = self.control(
            STANDARD_IN,
            GET_DESCRIPTOR,
            REPORT_DESCRIPTOR_TYPE << 8,
            &mut buffer,
            false,
        )?;
        buffer.truncate(read);
        Ok(buffer)
    }

    fn interrupt_in(&self, buffer: &mut [u8], timeout: i32) -> Result<Option<usize>, String> {
        let app = android_app()?;
        with_activity(&app, |env, _| {
            let array = env.new_byte_array(buffer.len())?;
            let read = env
                .call_method(
                    self.connection.as_obj(),
                    jni_str!("bulkTransfer"),
                    jni_sig!("(Landroid/hardware/usb/UsbEndpoint;[BII)I"),
                    &[
                        JValue::Object(self.endpoint_in.as_obj()),
                        JValue::Object(array.as_ref()),
                        JValue::Int(buffer.len() as i32),
                        JValue::Int(timeout),
                    ],
                )?
                .i()?;
            // `bulkTransfer` reports a timeout and a failure with the same -1,
            // which is why the parent module asks separately whether the device
            // is still there rather than reading this as a disconnect.
            if read <= 0 {
                return Ok(None);
            }
            let read = (read as usize).min(buffer.len());
            let mut signed = vec![0i8; read];
            array.get_region(env, 0, &mut signed)?;
            for (slot, byte) in buffer.iter_mut().zip(signed) {
                *slot = byte as u8;
            }
            Ok(Some(read))
        })
    }

    fn interrupt_out(&self, data: &[u8], timeout: i32) -> Result<bool, String> {
        let Some(endpoint) = &self.endpoint_out else {
            return Ok(false);
        };
        let app = android_app()?;
        with_activity(&app, |env, _| {
            let signed = data.iter().map(|byte| *byte as i8).collect::<Vec<_>>();
            let array = env.new_byte_array(signed.len())?;
            array.set_region(env, 0, &signed)?;
            let written = env
                .call_method(
                    self.connection.as_obj(),
                    jni_str!("bulkTransfer"),
                    jni_sig!("(Landroid/hardware/usb/UsbEndpoint;[BII)I"),
                    &[
                        JValue::Object(endpoint.as_obj()),
                        JValue::Object(array.as_ref()),
                        JValue::Int(signed.len() as i32),
                        JValue::Int(timeout),
                    ],
                )?
                .i()?;
            Ok(written)
        })
        .and_then(|written| {
            if written < 0 {
                return Err(format!(
                    "the USB interrupt transfer was refused by the device ({written})"
                ));
            }
            Ok(true)
        })
    }

    fn set_report(&self, report_type: u8, report_id: u8, data: &[u8]) -> Result<(), String> {
        let mut payload = data.to_vec();
        self.control(
            CLASS_OUT,
            SET_REPORT,
            (i32::from(report_type) << 8) | i32::from(report_id),
            &mut payload,
            true,
        )?;
        Ok(())
    }

    fn get_report(
        &self,
        report_type: u8,
        report_id: u8,
        buffer: &mut [u8],
    ) -> Result<usize, String> {
        self.control(
            CLASS_IN,
            GET_REPORT,
            (i32::from(report_type) << 8) | i32::from(report_id),
            buffer,
            false,
        )
    }

    fn attached(&self) -> Result<bool, String> {
        let app = android_app()?;
        with_activity(&app, |env, activity| {
            let manager = usb_manager(env, activity)?;
            Ok(!device_by_name(env, &manager, &self.device_name)?.is_null())
        })
    }
}

impl Drop for InterfaceConnection {
    /// Releases the claim and closes the connection, on whatever thread is
    /// dropping it.
    ///
    /// That thread is the device's own worker, which is exactly where this
    /// belongs: the interface goes back to the kernel driver it was taken from
    /// at the moment the last read stops, rather than whenever a garbage
    /// collector notices. A failure here has nowhere to be reported and nothing
    /// that could act on it — the process is either closing the device or
    /// losing it — so it is discarded rather than logged into a frame turn that
    /// may no longer exist.
    fn drop(&mut self) {
        let Ok(app) = android_app() else {
            return;
        };
        let _ = with_activity(&app, |env, _| {
            env.call_method(
                self.connection.as_obj(),
                jni_str!("releaseInterface"),
                jni_sig!("(Landroid/hardware/usb/UsbInterface;)Z"),
                &[JValue::Object(self.interface.as_obj())],
            )?;
            env.call_method(
                self.connection.as_obj(),
                jni_str!("close"),
                jni_sig!("()V"),
                &[],
            )?;
            Ok(())
        });
    }
}
