//! JavaScript-engine-independent interfaces.
//!
//! The types in this crate deliberately contain no Node-API, Bun, or
//! JavaScriptCore handles.  Bridge crates can therefore be compiled and tested
//! without selecting a JavaScript host.

pub mod timers;

use std::error::Error;
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

/// Stable identifier stored as opaque data on a native JavaScript object.
///
/// DOM wrappers use this for their generational node handle.  The JavaScript
/// engine must not inspect or reinterpret the value.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ExternalId(pub u64);

/// Broad JavaScript value categories used for checked conversions.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JsType {
    /// The `undefined` singleton.
    Undefined,
    /// The `null` singleton.
    Null,
    /// A boolean.
    Boolean,
    /// A number.
    Number,
    /// A string.
    String,
    /// An ordinary object, including native class instances.
    Object,
    /// An array.
    Array,
    /// A typed array.
    TypedArray,
    /// A callable function.
    Function,
}

/// Element representation of a JavaScript typed array.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TypedArrayKind {
    /// `Int8Array`.
    Int8,
    /// `Uint8Array`.
    Uint8,
    /// `Uint8ClampedArray`.
    Uint8Clamped,
    /// `Int16Array`.
    Int16,
    /// `Uint16Array`.
    Uint16,
    /// `Int32Array`.
    Int32,
    /// `Uint32Array`.
    Uint32,
    /// `Float32Array`.
    Float32,
    /// `Float64Array`.
    Float64,
    /// `BigInt64Array`.
    BigInt64,
    /// `BigUint64Array`.
    BigUint64,
}

impl TypedArrayKind {
    /// Returns the size of one element in bytes.
    pub const fn element_size(self) -> usize {
        match self {
            Self::Int8 | Self::Uint8 | Self::Uint8Clamped => 1,
            Self::Int16 | Self::Uint16 => 2,
            Self::Int32 | Self::Uint32 | Self::Float32 => 4,
            Self::Float64 | Self::BigInt64 | Self::BigUint64 => 8,
        }
    }
}

/// Owned typed-array contents copied across the engine boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TypedArray {
    /// JavaScript typed-array constructor represented by the bytes.
    pub kind: TypedArrayKind,
    /// Native-endian element bytes.
    pub bytes: Vec<u8>,
}

impl TypedArray {
    /// Creates validated typed-array contents.
    pub fn new(kind: TypedArrayKind, bytes: Vec<u8>) -> Result<Self, JsError> {
        if !bytes.len().is_multiple_of(kind.element_size()) {
            return Err(JsError::new(format!(
                "{} bytes cannot contain whole {kind:?} elements",
                bytes.len()
            )));
        }
        Ok(Self { kind, bytes })
    }

    /// Returns the number of elements represented by the byte buffer.
    pub fn len(&self) -> usize {
        self.bytes.len() / self.kind.element_size()
    }

    /// Reports whether the typed array has no elements.
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }
}

/// An exception or host-boundary failure reported by a JavaScript engine.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JsError {
    message: String,
    stack: Option<String>,
}

impl JsError {
    /// Creates an error without a JavaScript stack.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            stack: None,
        }
    }

    /// Attaches the originating JavaScript stack to an error.
    pub fn with_stack(message: impl Into<String>, stack: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            stack: Some(stack.into()),
        }
    }

    /// Returns the human-readable exception message.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Returns the JavaScript stack when the engine supplied one.
    pub fn stack(&self) -> Option<&str> {
        self.stack.as_deref()
    }
}

impl fmt::Display for JsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)?;
        if let Some(stack) = &self.stack {
            write!(formatter, "\n{stack}")?;
        }
        Ok(())
    }
}

impl Error for JsError {}

/// Arguments supplied by the engine when JavaScript invokes a native callback.
#[derive(Clone, Debug)]
pub struct NativeCall<V> {
    /// Receiver (`this`) for the invocation.
    pub this: V,
    /// Positional arguments supplied by JavaScript.
    pub arguments: Vec<V>,
    /// Opaque native data attached to the receiver, when present.
    pub external: Option<ExternalId>,
}

impl<V> NativeCall<V> {
    /// Returns a positional argument, naming the missing one when it is absent.
    pub fn argument(&self, index: usize, name: &str) -> Result<&V, JsError> {
        self.arguments
            .get(index)
            .ok_or_else(|| JsError::new(format!("missing {name}")))
    }
}

/// A host function callable by JavaScript.
///
/// Returning [`JsError`] must throw a catchable JavaScript exception.
pub type NativeCallback<V> = Box<dyn FnMut(NativeCall<V>) -> Result<V, JsError> + 'static>;

/// A method installed on a native class prototype.
pub struct NativeMethod<V> {
    /// JavaScript-visible method name.
    pub name: String,
    /// Method implementation.
    pub callback: NativeCallback<V>,
}

impl<V> NativeMethod<V> {
    /// Creates a native prototype method.
    pub fn new(name: impl Into<String>, callback: NativeCallback<V>) -> Self {
        Self {
            name: name.into(),
            callback,
        }
    }
}

/// Runtime-neutral description of a native JavaScript class.
pub struct NativeClass<V> {
    /// JavaScript-visible constructor and prototype name.
    pub name: String,
    /// Prototype methods installed when the class is registered.
    pub methods: Vec<NativeMethod<V>>,
}

impl<V> NativeClass<V> {
    /// Creates an empty class definition.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            methods: Vec::new(),
        }
    }

    /// Adds a method to the class prototype.
    pub fn with_method(mut self, method: NativeMethod<V>) -> Self {
        self.methods.push(method);
        self
    }
}

/// Result of allowing the host event loop to advance once.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LoopTurn {
    /// No task was ready.
    Idle,
    /// At least one task ran.
    Progress,
}

/// Boundary implemented by every JavaScript host.
///
/// Phase 1 implements this over Bun/Node-API. Phase 2 implements it over an
/// embedded JavaScriptCore host. Bridge code depends only on this trait.
pub trait JsEngine {
    /// Opaque, engine-owned JavaScript value handle.
    type Value: Clone;
    /// Opaque weak reference to a JavaScript value.
    type WeakRef;
    /// Opaque registered native-class handle.
    type Class;

    /// Re-enters the engine from a value it handed to a native callback.
    ///
    /// A native callback is owned by the engine, so it cannot also borrow it,
    /// yet almost every callback needs to build its return value. Both hosts
    /// carry the owning context inside the value handle itself, so the engine
    /// is recoverable from any argument — including [`NativeCall::this`], which
    /// is present even for a zero-argument call. Bridge code uses this instead
    /// of capturing a host-specific environment pointer.
    fn from_value(value: &Self::Value) -> Self
    where
        Self: Sized;

    /// Creates `undefined`.
    fn undefined(&mut self) -> Self::Value;
    /// Creates `null`.
    fn null(&mut self) -> Self::Value;
    /// Creates a boolean.
    fn boolean(&mut self, value: bool) -> Self::Value;
    /// Creates a number.
    fn number(&mut self, value: f64) -> Self::Value;
    /// Creates a string.
    fn string(&mut self, value: &str) -> Result<Self::Value, JsError>;
    /// Creates an empty ordinary object.
    fn object(&mut self) -> Result<Self::Value, JsError>;
    /// Creates an array from values.
    fn array(&mut self, values: &[Self::Value]) -> Result<Self::Value, JsError>;
    /// Creates a typed array by copying validated native-endian bytes.
    fn typed_array(&mut self, value: &TypedArray) -> Result<Self::Value, JsError>;
    /// Returns the broad runtime type of a value.
    fn value_type(&mut self, value: &Self::Value) -> Result<JsType, JsError>;
    /// Converts a value using JavaScript boolean coercion.
    fn to_boolean(&mut self, value: &Self::Value) -> Result<bool, JsError>;
    /// Converts a value using JavaScript numeric coercion.
    fn to_number(&mut self, value: &Self::Value) -> Result<f64, JsError>;
    /// Converts a value using JavaScript string coercion.
    fn to_string(&mut self, value: &Self::Value) -> Result<String, JsError>;
    /// Copies an array's elements into Rust.
    fn to_array(&mut self, value: &Self::Value) -> Result<Vec<Self::Value>, JsError>;
    /// Copies a typed array's type and contents into Rust.
    fn to_typed_array(&mut self, value: &Self::Value) -> Result<TypedArray, JsError>;

    /// Reads an object property.
    fn get_property(&mut self, object: &Self::Value, name: &str) -> Result<Self::Value, JsError>;
    /// Writes an object property.
    fn set_property(
        &mut self,
        object: &Self::Value,
        name: &str,
        value: &Self::Value,
    ) -> Result<(), JsError>;
    /// Installs a value on the JavaScript global object.
    fn set_global(&mut self, name: &str, value: &Self::Value) -> Result<(), JsError>;

    /// Creates a JavaScript function backed by a Rust callback.
    fn define_function(
        &mut self,
        name: &str,
        callback: NativeCallback<Self::Value>,
    ) -> Result<Self::Value, JsError>;
    /// Defines a native function and installs it on the global object under the
    /// same name.
    ///
    /// Every host function the bridge installs is a global named exactly as it
    /// was defined, so the two names are one fact rather than two: spelling it
    /// once is what stops a rename from leaving the function reachable under
    /// its old name, or under no name at all.
    fn define_global_function(
        &mut self,
        name: &str,
        callback: NativeCallback<Self::Value>,
    ) -> Result<(), JsError> {
        let function = self.define_function(name, callback)?;
        self.set_global(name, &function)
    }
    /// Invokes a callable value and captures any thrown exception.
    fn call(
        &mut self,
        function: &Self::Value,
        this: Option<&Self::Value>,
        arguments: &[Self::Value],
    ) -> Result<Self::Value, JsError>;
    /// Invokes one host macrotask and performs its microtask checkpoint.
    ///
    /// Hosts with a dedicated callback primitive should override this method;
    /// embedded engines may use the default call followed by an explicit drain.
    fn call_macrotask(
        &mut self,
        function: &Self::Value,
        this: Option<&Self::Value>,
        arguments: &[Self::Value],
    ) -> Result<Self::Value, JsError> {
        let result = self.call(function, this, arguments)?;
        self.drain_microtasks()?;
        Ok(result)
    }

    /// Registers a native constructor and prototype.
    fn register_class(
        &mut self,
        definition: NativeClass<Self::Value>,
    ) -> Result<Self::Class, JsError>;
    /// Creates an instance and attaches opaque external data.
    ///
    /// The finalizer is invoked exactly once if provided, after the JavaScript
    /// object becomes unreachable.
    fn instantiate(
        &mut self,
        class: &Self::Class,
        external: ExternalId,
        finalizer: Option<Box<dyn FnOnce(ExternalId) + 'static>>,
    ) -> Result<Self::Value, JsError>;
    /// Reads opaque external data, rejecting values not created by
    /// [`JsEngine::instantiate`].
    fn external_id(&mut self, value: &Self::Value) -> Result<ExternalId, JsError>;

    /// Detaches an `ArrayBuffer`, leaving it zero-length and unusable.
    ///
    /// What `postMessage` does to a buffer in its transfer list. There is no way
    /// to express it in JavaScript, and a transfer that copied instead would be
    /// a different operation wearing the same name — the sender would go on
    /// reading bytes the receiver now owns. A host that cannot detach must say
    /// so rather than pretend, which is what the default does.
    fn detach_array_buffer(&mut self, buffer: &Self::Value) -> Result<(), JsError> {
        let _ = buffer;
        Err(JsError::new(
            "this JavaScript host cannot transfer an ArrayBuffer",
        ))
    }

    /// Asks the engine to abandon running JavaScript once `stop` is set.
    ///
    /// How `worker.terminate()` reaches a worker that is inside a loop rather
    /// than between turns. A host without an interrupt hook keeps the default:
    /// the flag is still honoured, but only where the worker's own event loop
    /// can see it, so a script that never yields runs to its end.
    fn set_interrupt_flag(&mut self, stop: Arc<AtomicBool>) -> Result<(), JsError> {
        let _ = stop;
        Ok(())
    }

    /// Creates a weak reference that does not keep its target alive.
    fn downgrade(&mut self, value: &Self::Value) -> Result<Self::WeakRef, JsError>;
    /// Returns the live weak-reference target, or `None` after collection.
    fn upgrade(&mut self, reference: &Self::WeakRef) -> Result<Option<Self::Value>, JsError>;

    /// Evaluates a classic script with the supplied source identifier.
    fn evaluate_script(&mut self, source: &str, filename: &str) -> Result<Self::Value, JsError>;
    /// Starts ECMAScript module evaluation.
    ///
    /// Embedded engines may return the namespace directly. Hosts such as Bun
    /// return the dynamic-import promise because Node-API cannot synchronously
    /// drain the host's module graph.
    fn evaluate_module(&mut self, source: &str, identifier: &str) -> Result<Self::Value, JsError>;
    /// Runs queued microtasks to quiescence and returns the number processed.
    fn drain_microtasks(&mut self) -> Result<usize, JsError>;
    /// Gives the JavaScript host event loop one non-blocking turn.
    fn pump_event_loop(&mut self) -> Result<LoopTurn, JsError>;

    /// Collects the heap now, rather than waiting for the engine's threshold.
    ///
    /// Asked for when the operating system says it is short of memory. The
    /// default is to do nothing, and that is the right answer for an engine
    /// whose only targets never receive such a warning: no desktop winit
    /// backend delivers one, so JavaScriptCore and Node-API keep the default.
    /// QuickJS implements it, because QuickJS is what Android and iOS host and
    /// they are the two platforms that ask (issue #146).
    fn collect_garbage(&mut self) -> Result<(), JsError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_arrays_require_whole_elements() {
        let error = TypedArray::new(TypedArrayKind::Float64, vec![0; 7]).unwrap_err();
        assert!(error.message().contains("whole Float64 elements"));

        let array = TypedArray::new(TypedArrayKind::Float64, vec![0; 16]).unwrap();
        assert_eq!(array.len(), 2);
        assert!(!array.is_empty());
    }

    #[test]
    fn errors_preserve_optional_javascript_stacks() {
        let error = JsError::with_stack("boom", "at app.js:1:1");
        assert_eq!(error.message(), "boom");
        assert_eq!(error.stack(), Some("at app.js:1:1"));
        assert_eq!(error.to_string(), "boom\nat app.js:1:1");
    }
}
