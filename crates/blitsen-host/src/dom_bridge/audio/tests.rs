use super::*;

fn dispatch(host: &AudioHost, operation: &str, arguments: &[&str]) -> Result<Value, JsError> {
    let arguments = arguments
        .iter()
        .map(|argument| (*argument).to_owned())
        .collect::<Vec<_>>();
    host.dispatch(operation, &arguments)
}

fn id(value: &Value) -> String {
    value.as_u64().expect("node id").to_string()
}

fn error(host: &AudioHost, operation: &str, arguments: &[&str]) -> String {
    dispatch(host, operation, arguments)
        .expect_err("operation should fail")
        .message()
        .to_owned()
}

#[test]
fn dispatch_routes_each_audio_operation_family() {
    let host = AudioHost::new(true, None);
    assert_eq!(dispatch(&host, "mode", &["offline"]), Ok(Value::Null));

    let gain = id(&dispatch(&host, "create", &["gain"]).unwrap());
    let panner = id(&dispatch(&host, "create", &["panner"]).unwrap());
    let source = id(&dispatch(&host, "create", &["source"]).unwrap());
    assert_eq!(
        dispatch(&host, "connect", &[&gain, &panner]),
        Ok(Value::Null)
    );

    assert_eq!(
        dispatch(&host, "paramSet", &[&gain, "gain", "0.25"]),
        Ok(Value::Null)
    );
    assert_eq!(
        dispatch(&host, "paramValue", &[&gain, "gain"]),
        Ok(json!(0.25))
    );
    for arguments in [
        vec![&gain, "gain", "setValueAtTime", "0.5", "0"],
        vec![&gain, "gain", "linearRampToValueAtTime", "0.75", "0.1"],
        vec![&gain, "gain", "exponentialRampToValueAtTime", "0.5", "0.2"],
        vec![&gain, "gain", "setTargetAtTime", "0.25", "0.3", "0.1"],
        vec![&gain, "gain", "cancelScheduledValues", "0", "0.4"],
    ] {
        assert_eq!(
            dispatch(&host, "paramSchedule", &arguments),
            Ok(Value::Null)
        );
    }

    let buffer_id = 500;
    host.buffers.lock().insert(
        buffer_id,
        AudioBuffer::from(vec![vec![0.0f32; 16]], OFFLINE_SAMPLE_RATE),
    );
    let buffer_id = buffer_id.to_string();
    assert_eq!(
        dispatch(&host, "sourceBuffer", &[&source, &buffer_id]),
        Ok(Value::Null)
    );
    assert_eq!(
        dispatch(&host, "sourceLoop", &[&source, "1"]),
        Ok(Value::Null)
    );
    assert_eq!(
        dispatch(&host, "sourceStart", &[&source, "0", "0"]),
        Ok(Value::Null)
    );
    assert_eq!(
        dispatch(&host, "sourceStop", &[&source, "0.2"]),
        Ok(Value::Null)
    );

    assert_eq!(dispatch(&host, "disconnect", &[&gain]), Ok(Value::Null));
    assert_eq!(dispatch(&host, "release", &[&panner]), Ok(Value::Null));
}

#[test]
fn dispatch_preserves_context_and_protocol_errors() {
    let host = AudioHost::new(true, None);
    assert_eq!(
        error(&host, "notAnAudioOperation", &[]),
        "unknown audio operation: notAnAudioOperation"
    );
    assert_eq!(error(&host, "create", &[]), "missing audio argument 0");
    assert_eq!(
        error(&host, "release", &["not-a-number"]),
        "invalid audio argument 0"
    );
    assert_eq!(
        error(&host, "mode", &["impossible"]),
        "unknown audio mode: impossible"
    );
    assert_eq!(
        error(&host, "create", &["oscillator"]),
        "unknown audio node: oscillator"
    );

    dispatch(&host, "context", &[]).unwrap();
    assert_eq!(
        error(&host, "mode", &[]),
        "the audio context is already open"
    );
}

#[test]
fn dispatch_preserves_param_source_and_buffer_error_order() {
    let host = AudioHost::new(true, None);
    let gain = id(&dispatch(&host, "create", &["gain"]).unwrap());

    // `paramSet` validates the assigned value before looking up the node.
    assert_eq!(
        error(&host, "paramSet", &["bad-id", "gain", "bad-value"]),
        "invalid audio argument 2"
    );
    assert_eq!(
        error(&host, "paramValue", &[&gain, "frequency"]),
        "no audio parameter named frequency"
    );
    assert_eq!(
        error(
            &host,
            "paramSchedule",
            &[&gain, "gain", "unknownSchedule", "1", "0"],
        ),
        "unknown parameter schedule: unknownSchedule"
    );
    assert_eq!(
        error(
            &host,
            "paramSchedule",
            &[&gain, "gain", "setTargetAtTime", "1", "0"],
        ),
        "missing audio argument 5"
    );

    // A wrong node kind is diagnosed before source-only arguments are read.
    assert_eq!(
        error(&host, "sourceLoop", &[&gain, "bad-loop"]),
        "only a buffer source loops"
    );
    assert_eq!(
        error(&host, "sourceStart", &[&gain, "bad-when", "bad-offset"]),
        "only a buffer source can be started"
    );
    // Buffer lookup precedes the source-kind check for assignment.
    assert_eq!(
        error(&host, "sourceBuffer", &[&gain, "999"]),
        "the audio buffer has been released"
    );
}

#[test]
fn audio_file_loads_are_confined_to_the_application() {
    let root = tempfile::tempdir().unwrap();
    let entrypoint = root.path().join("index.html");
    std::fs::write(&entrypoint, "<p>audio</p>").unwrap();
    let files = crate::app::AppFiles::directory(&entrypoint).unwrap();
    let host = AudioHost::new(true, Some(files.reader()));

    let outside = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(outside.path(), b"not audio").unwrap();
    let url = url::Url::from_file_path(outside.path()).unwrap();
    host.start_load(url.as_str()).unwrap();

    while host.pending() && host.shared.decoded.lock().is_empty() {
        std::thread::yield_now();
    }
    let result = host.poll();
    assert_eq!(
        result["decoded"][0]["error"],
        format!(
            "an audio source is a file this application shipped, or an http or https URL, not {url}"
        )
    );
}
