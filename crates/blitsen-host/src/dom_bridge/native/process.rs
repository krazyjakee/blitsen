use blitsen_js::{JsEngine, JsError};

#[cfg(not(any(target_os = "android", target_os = "ios")))]
use super::super::{argument, byte_argument, json_value};

/// Where a shipped sidecar executable is looked for: beside the running
/// executable (inside `Contents/MacOS` for a macOS bundle), then in the
/// application directory for a directory run. The first that exists wins; when
/// neither does, the path beside the executable is returned so the spawn reports
/// `NotFoundError` naming where the sidecar was expected.
#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn sidecar_path(
    name: &str,
    executable: Option<&std::path::Path>,
    application_root: Option<&std::path::Path>,
) -> Result<std::path::PathBuf, JsError> {
    let valid = !name.is_empty()
        && name.len() <= 255
        && name
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    if !valid {
        return Err(JsError::new(format!(
            "{name:?} is not a sidecar name: use the shipped executable's file name"
        )));
    }
    let file = if cfg!(windows) && !name.to_ascii_lowercase().ends_with(".exe") {
        format!("{name}.exe")
    } else {
        name.to_owned()
    };
    let candidates: Vec<std::path::PathBuf> = executable
        .and_then(std::path::Path::parent)
        .into_iter()
        .chain(application_root)
        .map(|directory| directory.join(&file))
        .collect();
    candidates
        .iter()
        .find(|candidate| candidate.is_file())
        .or(candidates.first())
        .cloned()
        .ok_or_else(|| {
            JsError::new("cannot locate a sidecar without an executable or application directory")
        })
}

#[cfg(all(test, not(any(target_os = "android", target_os = "ios"))))]
mod sidecar_tests {
    use super::sidecar_path;

    #[test]
    fn sidecars_resolve_beside_the_executable_then_the_application_directory() {
        let temporary =
            std::env::temp_dir().join(format!("blitsen-sidecar-{}", std::process::id()));
        let (bin, app) = (temporary.join("bin"), temporary.join("app"));
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::create_dir_all(&app).unwrap();
        let file = if cfg!(windows) {
            "helper.exe"
        } else {
            "helper"
        };
        let executable = bin.join("app");
        // Neither exists: the path beside the executable is named.
        assert_eq!(
            sidecar_path("helper", Some(&executable), Some(&app)).unwrap(),
            bin.join(file)
        );
        std::fs::write(app.join(file), b"").unwrap();
        assert_eq!(
            sidecar_path("helper", Some(&executable), Some(&app)).unwrap(),
            app.join(file)
        );
        std::fs::write(bin.join(file), b"").unwrap();
        assert_eq!(
            sidecar_path("helper", Some(&executable), Some(&app)).unwrap(),
            bin.join(file)
        );
        for bad in ["", "../helper", "a/b", ".hidden", "x\\y", "sp ace"] {
            assert!(
                sidecar_path(bad, Some(&executable), None).is_err(),
                "{bad:?}"
            );
        }
        // Never turn a sidecar into a bare command that the supervisor searches on PATH.
        assert!(sidecar_path("helper", None, None).is_err());
        std::fs::remove_dir_all(&temporary).unwrap();
    }
}

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
        command: Option<String>,
        sidecar: Option<String>,
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
            let program = match (spec.command, spec.sidecar) {
                (Some(command), None) => command,
                (None, Some(name)) => sidecar_path(
                    &name,
                    std::env::current_exe().ok().as_deref(),
                    crate::app::application_root().as_deref(),
                )?
                .into_os_string()
                .into_string()
                .map_err(|_| JsError::new("the sidecar's location is not valid Unicode"))?,
                _ => return Err(JsError::new("spawn takes a command or a sidecar, not both")),
            };
            let request = SpawnRequest {
                program,
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
