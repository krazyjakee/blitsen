//! Dynamically resolved JavaScriptCore C API.

use std::ffi::{c_char, c_int, c_uint, c_void};

use libloading::Library;

use crate::Error;

pub(crate) type JsClassRef = *mut c_void;
pub(crate) type JsContextRef = *const c_void;
pub(crate) type JsGlobalContextRef = *const c_void;
pub(crate) type JsObjectRef = *mut c_void;
pub(crate) type JsStringRef = *mut c_void;
pub(crate) type JsValueRef = *const c_void;

pub(crate) type CallAsFunction = unsafe extern "C" fn(
    JsContextRef,
    JsObjectRef,
    JsObjectRef,
    usize,
    *const JsValueRef,
    *mut JsValueRef,
) -> JsValueRef;
pub(crate) type Finalize = unsafe extern "C" fn(JsObjectRef);

#[repr(C)]
pub(crate) struct ClassDefinition {
    pub(crate) version: c_int,
    pub(crate) attributes: c_uint,
    pub(crate) class_name: *const c_char,
    pub(crate) parent_class: JsClassRef,
    pub(crate) static_values: *const c_void,
    pub(crate) static_functions: *const c_void,
    pub(crate) initialize: Option<unsafe extern "C" fn(JsContextRef, JsObjectRef)>,
    pub(crate) finalize: Option<Finalize>,
    pub(crate) has_property:
        Option<unsafe extern "C" fn(JsContextRef, JsObjectRef, JsStringRef) -> bool>,
    pub(crate) get_property: Option<
        unsafe extern "C" fn(JsContextRef, JsObjectRef, JsStringRef, *mut JsValueRef) -> JsValueRef,
    >,
    pub(crate) set_property: Option<
        unsafe extern "C" fn(
            JsContextRef,
            JsObjectRef,
            JsStringRef,
            JsValueRef,
            *mut JsValueRef,
        ) -> bool,
    >,
    pub(crate) delete_property: Option<
        unsafe extern "C" fn(JsContextRef, JsObjectRef, JsStringRef, *mut JsValueRef) -> bool,
    >,
    pub(crate) get_property_names:
        Option<unsafe extern "C" fn(JsContextRef, JsObjectRef, *mut c_void)>,
    pub(crate) call_as_function: Option<CallAsFunction>,
    pub(crate) call_as_constructor: Option<
        unsafe extern "C" fn(
            JsContextRef,
            JsObjectRef,
            usize,
            *const JsValueRef,
            *mut JsValueRef,
        ) -> JsObjectRef,
    >,
    pub(crate) has_instance: Option<
        unsafe extern "C" fn(JsContextRef, JsObjectRef, JsValueRef, *mut JsValueRef) -> bool,
    >,
    pub(crate) convert_to_type: Option<
        unsafe extern "C" fn(JsContextRef, JsObjectRef, c_uint, *mut JsValueRef) -> JsValueRef,
    >,
}

impl ClassDefinition {
    pub(crate) fn named(name: *const c_char) -> Self {
        Self {
            version: 0,
            attributes: 0,
            class_name: name,
            parent_class: std::ptr::null_mut(),
            static_values: std::ptr::null(),
            static_functions: std::ptr::null(),
            initialize: None,
            finalize: None,
            has_property: None,
            get_property: None,
            set_property: None,
            delete_property: None,
            get_property_names: None,
            call_as_function: None,
            call_as_constructor: None,
            has_instance: None,
            convert_to_type: None,
        }
    }
}

macro_rules! api {
    ($($field:ident: $type:ty => $symbol:literal),+ $(,)?) => {
        pub(crate) struct Functions {
            $(pub(crate) $field: $type,)+
            pub(crate) load_and_evaluate_module_from_source: Option<unsafe extern "C" fn(
                JsContextRef, JsStringRef, JsStringRef, c_int, *mut JsValueRef,
            )>,
            pub(crate) set_module_loader_functions: Option<unsafe extern "C" fn(
                JsGlobalContextRef, JsObjectRef, JsObjectRef,
            )>,
        }

        impl Functions {
            pub(crate) unsafe fn load(library: &Library) -> Result<Self, Error> {
                unsafe fn required<T: Copy>(
                    library: &Library,
                    name: &'static [u8],
                    display: &'static str,
                ) -> Result<T, Error> {
                    // SAFETY: the caller supplies the exact C declaration for the named symbol.
                    unsafe { library.get::<T>(name) }
                        .map(|symbol| *symbol)
                        .map_err(|source| Error::MissingSymbol { symbol: display, source })
                }

                Ok(Self {
                    $($field: unsafe { required::<$type>(library, concat!($symbol, "\0").as_bytes(), $symbol)? },)+
                    // Bun's WebKit fork exposes this module entry point. System
                    // JavaScriptCore is still useful for the portable C-API tests.
                    load_and_evaluate_module_from_source: unsafe {
                        library
                            .get::<unsafe extern "C" fn(
                                JsContextRef, JsStringRef, JsStringRef, c_int, *mut JsValueRef,
                            )>(b"JSLoadAndEvaluateModuleFromSource\0")
                            .ok()
                            .map(|symbol| *symbol)
                    },
                    // The module loader hook Blitsen's pinned build adds. It
                    // takes two ordinary JavaScript functions rather than C
                    // callbacks, because the loader already works in JSValues
                    // and because it keeps this side of the ABI to one symbol.
                    // See docs/JSC.md, "Module loader contract".
                    set_module_loader_functions: unsafe {
                        library
                            .get::<unsafe extern "C" fn(
                                JsGlobalContextRef, JsObjectRef, JsObjectRef,
                            )>(b"JSGlobalContextSetModuleLoaderFunctions\0")
                            .ok()
                            .map(|symbol| *symbol)
                    },
                })
            }
        }
    };
}

api! {
    global_context_create: unsafe extern "C" fn(JsClassRef) -> JsGlobalContextRef => "JSGlobalContextCreate",
    string_create_utf8: unsafe extern "C" fn(*const c_char) -> JsStringRef => "JSStringCreateWithUTF8CString",
    string_release: unsafe extern "C" fn(JsStringRef) -> () => "JSStringRelease",
    string_max_utf8: unsafe extern "C" fn(JsStringRef) -> usize => "JSStringGetMaximumUTF8CStringSize",
    string_get_utf8: unsafe extern "C" fn(JsStringRef, *mut c_char, usize) -> usize => "JSStringGetUTF8CString",
    evaluate_script: unsafe extern "C" fn(JsContextRef, JsStringRef, JsObjectRef, JsStringRef, c_int, *mut JsValueRef) -> JsValueRef => "JSEvaluateScript",
    context_global: unsafe extern "C" fn(JsContextRef) -> JsObjectRef => "JSContextGetGlobalObject",
    value_undefined: unsafe extern "C" fn(JsContextRef) -> JsValueRef => "JSValueMakeUndefined",
    value_null: unsafe extern "C" fn(JsContextRef) -> JsValueRef => "JSValueMakeNull",
    value_boolean: unsafe extern "C" fn(JsContextRef, bool) -> JsValueRef => "JSValueMakeBoolean",
    value_number: unsafe extern "C" fn(JsContextRef, f64) -> JsValueRef => "JSValueMakeNumber",
    value_string: unsafe extern "C" fn(JsContextRef, JsStringRef) -> JsValueRef => "JSValueMakeString",
    value_type: unsafe extern "C" fn(JsContextRef, JsValueRef) -> c_uint => "JSValueGetType",
    value_to_boolean: unsafe extern "C" fn(JsContextRef, JsValueRef) -> bool => "JSValueToBoolean",
    value_to_number: unsafe extern "C" fn(JsContextRef, JsValueRef, *mut JsValueRef) -> f64 => "JSValueToNumber",
    value_to_string: unsafe extern "C" fn(JsContextRef, JsValueRef, *mut JsValueRef) -> JsStringRef => "JSValueToStringCopy",
    value_to_object: unsafe extern "C" fn(JsContextRef, JsValueRef, *mut JsValueRef) -> JsObjectRef => "JSValueToObject",
    value_is_array: unsafe extern "C" fn(JsContextRef, JsValueRef) -> bool => "JSValueIsArray",
    value_typed_array_type: unsafe extern "C" fn(JsContextRef, JsValueRef, *mut JsValueRef) -> c_uint => "JSValueGetTypedArrayType",
    value_protect: unsafe extern "C" fn(JsContextRef, JsValueRef) -> () => "JSValueProtect",
    value_unprotect: unsafe extern "C" fn(JsContextRef, JsValueRef) -> () => "JSValueUnprotect",
    object_make: unsafe extern "C" fn(JsContextRef, JsClassRef, *mut c_void) -> JsObjectRef => "JSObjectMake",
    object_make_array: unsafe extern "C" fn(JsContextRef, usize, *const JsValueRef, *mut JsValueRef) -> JsObjectRef => "JSObjectMakeArray",
    object_make_typed_array: unsafe extern "C" fn(JsContextRef, c_uint, usize, *mut JsValueRef) -> JsObjectRef => "JSObjectMakeTypedArray",
    object_typed_array_bytes: unsafe extern "C" fn(JsContextRef, JsObjectRef, *mut JsValueRef) -> *mut c_void => "JSObjectGetTypedArrayBytesPtr",
    object_typed_array_byte_length: unsafe extern "C" fn(JsContextRef, JsObjectRef, *mut JsValueRef) -> usize => "JSObjectGetTypedArrayByteLength",
    object_get_property: unsafe extern "C" fn(JsContextRef, JsObjectRef, JsStringRef, *mut JsValueRef) -> JsValueRef => "JSObjectGetProperty",
    object_set_property: unsafe extern "C" fn(JsContextRef, JsObjectRef, JsStringRef, JsValueRef, c_uint, *mut JsValueRef) -> () => "JSObjectSetProperty",
    object_get_index: unsafe extern "C" fn(JsContextRef, JsObjectRef, c_uint, *mut JsValueRef) -> JsValueRef => "JSObjectGetPropertyAtIndex",
    object_is_function: unsafe extern "C" fn(JsContextRef, JsObjectRef) -> bool => "JSObjectIsFunction",
    object_call: unsafe extern "C" fn(JsContextRef, JsObjectRef, JsObjectRef, usize, *const JsValueRef, *mut JsValueRef) -> JsValueRef => "JSObjectCallAsFunction",
    object_make_constructor: unsafe extern "C" fn(JsContextRef, JsClassRef, *const c_void) -> JsObjectRef => "JSObjectMakeConstructor",
    object_construct: unsafe extern "C" fn(JsContextRef, JsObjectRef, usize, *const JsValueRef, *mut JsValueRef) -> JsObjectRef => "JSObjectCallAsConstructor",
    object_get_private: unsafe extern "C" fn(JsObjectRef) -> *mut c_void => "JSObjectGetPrivate",
    object_set_private: unsafe extern "C" fn(JsObjectRef, *mut c_void) -> bool => "JSObjectSetPrivate",
    class_create: unsafe extern "C" fn(*const ClassDefinition) -> JsClassRef => "JSClassCreate",
    class_release: unsafe extern "C" fn(JsClassRef) -> () => "JSClassRelease",
    garbage_collect: unsafe extern "C" fn(JsContextRef) -> () => "JSGarbageCollect",
}
