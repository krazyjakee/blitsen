use blitsen_js::{JsEngine, JsError};

#[cfg(not(any(target_os = "android", target_os = "ios")))]
use super::super::{argument, byte_argument, json_value};

/// `blitsen/process` (#383): managed child processes.
///
/// Every command is handed to the platform's own supervisor threads and every
/// answer — a spawn settling, a write settling, a chunk of output, the exit —
/// crosses on a frame turn through one FIFO, so a tool that writes for an
/// hour never re-enters the application from the thread that read it. Output
/// crosses as raw bytes beside its JSON header, the way HID reports do: a
/// build log re-encoded into JSON and parsed back every frame would be the
/// stall this module exists to avoid.
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub(super) fn install<E: JsEngine + 'static>(engine: &mut E) -> Result<(), JsError> {
    use blitsen_js::{TypedArray, TypedArrayKind};
    use blitsen_platform::process::{self, Event, SpawnRequest, StdioMode, Stream};
    use serde::Deserialize;
    use serde_json::{Value, json};

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct SpawnSpec {
        command: String,
        args: Vec<String>,
        cwd: Option<String>,
        env: Vec<(String, Option<String>)>,
        inherit_env: bool,
        stdin: String,
        stdout: String,
        stderr: String,
    }

    fn mode(value: &str) -> Result<StdioMode, JsError> {
        match value {
            "piped" => Ok(StdioMode::Piped),
            "inherit" => Ok(StdioMode::Inherit),
            "null" => Ok(StdioMode::Null),
            other => Err(JsError::new(format!("{other:?} is not a stdio mode"))),
        }
    }

    fn id_argument<E: JsEngine>(
        engine: &mut E,
        call: &blitsen_js::NativeCall<E::Value>,
    ) -> Result<u64, JsError> {
        argument(engine, call, 0, "child process id")?
            .parse::<u64>()
            .map_err(|_| JsError::new("a child process id is a number"))
    }

    fn completion<T>(
        command_id: u64,
        result: Result<T, process::Failure>,
        value: impl FnOnce(T) -> Value,
    ) -> Value {
        match result {
            Ok(answer) => json!({
                "type": "completion", "commandId": command_id.to_string(),
                "value": value(answer), "error": null, "errorName": null,
            }),
            Err(failure) => json!({
                "type": "completion", "commandId": command_id.to_string(),
                "value": null, "error": failure.message(), "errorName": failure.name(),
            }),
        }
    }

    engine.define_global_function(
        "__blitsenNativeProcessSpawn",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            let spec: SpawnSpec =
                serde_json::from_str(&argument(&mut engine, &call, 0, "spawn options")?)
                    .map_err(|error| JsError::new(format!("malformed spawn options: {error}")))?;
            let request = SpawnRequest {
                program: spec.command,
                args: spec.args,
                cwd: spec.cwd.map(Into::into),
                env: spec.env,
                inherit_env: spec.inherit_env,
                stdin: mode(&spec.stdin)?,
                stdout: mode(&spec.stdout)?,
                stderr: mode(&spec.stderr)?,
            };
            engine.string(&process::spawn(request).to_string())
        }),
    )?;

    engine.define_global_function(
        "__blitsenNativeProcessWrite",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            let id = id_argument(&mut engine, &call)?;
            let data = byte_argument(&mut engine, call.argument(1, "stdin data")?, "stdin data")?;
            let command_id = process::write(id, data)
                .map_err(|error| JsError::new(error.message().to_owned()))?;
            engine.string(&command_id.to_string())
        }),
    )?;

    engine.define_global_function(
        "__blitsenNativeProcessCloseStdin",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            process::close_stdin(id_argument(&mut engine, &call)?);
            Ok(engine.undefined())
        }),
    )?;

    engine.define_global_function(
        "__blitsenNativeProcessKill",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            let id = id_argument(&mut engine, &call)?;
            let force = engine.to_boolean(call.argument(1, "force flag")?)?;
            process::kill(id, force);
            Ok(engine.undefined())
        }),
    )?;

    engine.define_global_function(
        "__blitsenNativeProcessPending",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            Ok(engine.boolean(process::pending()))
        }),
    )?;

    engine.define_global_function(
        "__blitsenNativeProcessDisposeAll",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            process::dispose_all();
            Ok(engine.undefined())
        }),
    )?;

    engine.define_global_function(
        "__blitsenNativeProcessTake",
        Box::new(move |call| {
            let mut engine = E::from_value(&call.this);
            let mut messages = Vec::new();
            for event in process::take() {
                let (value, data) = match event {
                    Event::Spawned { command_id, result } => (
                        completion(
                            command_id,
                            result,
                            |spawned| json!({ "id": spawned.id.to_string(), "pid": spawned.pid }),
                        ),
                        None,
                    ),
                    Event::Written { command_id, result } => {
                        (completion(command_id, result, |()| Value::Null), None)
                    }
                    Event::Output { id, stream, data } => (
                        json!({
                            "type": "output",
                            "id": id.to_string(),
                            "stream": match stream {
                                Stream::Stdout => "stdout",
                                Stream::Stderr => "stderr",
                            },
                        }),
                        Some(data),
                    ),
                    Event::Exited { id, status } => (
                        json!({
                            "type": "exit",
                            "id": id.to_string(),
                            "code": status.code,
                            "signal": status.signal,
                        }),
                        None,
                    ),
                };
                let object = engine.object()?;
                let json = json_value(&mut engine, &value)?;
                engine.set_property(&object, "json", &json)?;
                let data = match data {
                    Some(bytes) => {
                        engine.typed_array(&TypedArray::new(TypedArrayKind::Uint8, bytes)?)?
                    }
                    None => engine.null(),
                };
                engine.set_property(&object, "data", &data)?;
                messages.push(object);
            }
            engine.array(&messages)
        }),
    )
}

// A mobile application process may not spawn a developer tool, and has no
// executable to spawn one with.
#[cfg(any(target_os = "android", target_os = "ios"))]
pub(super) fn install<E: JsEngine + 'static>(_engine: &mut E) -> Result<(), JsError> {
    Ok(())
}
