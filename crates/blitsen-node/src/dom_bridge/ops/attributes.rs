//! Element attributes, and the form-control state that shadows them.

// These are slices of what was one `match`, so they share the helpers and
// imports their parent gathers rather than restating them.
use super::*;

pub(super) fn dispatch(
    runtime: &DomRuntime,
    dom: &mut BlitzDom,
    operation: &str,
    arguments: &[String],
) -> Answer {
    let value = match operation {
        "getAttribute" => Ok(dom
            .attribute(
                handle(runtime, arguments, 0)?,
                &DomName::attribute(
                    bridge_arg(arguments, 1, "attribute name")?.to_ascii_lowercase(),
                ),
            )
            .map_err(dom_error)?
            .map(Value::String)
            .unwrap_or(Value::Null)),
        "setAttribute" => {
            let node = handle(runtime, arguments, 0)?;
            let name = DomName::attribute(
                bridge_arg(arguments, 1, "attribute name")?.to_ascii_lowercase(),
            );
            dom.set_attribute(node, &name, bridge_arg(arguments, 2, "attribute value")?)
                .map_err(dom_error)?;
            Ok(Value::Null)
        }
        "removeAttribute" => {
            let node = handle(runtime, arguments, 0)?;
            let name = DomName::attribute(
                bridge_arg(arguments, 1, "attribute name")?.to_ascii_lowercase(),
            );
            dom.remove_attribute(node, &name).map_err(dom_error)?;
            Ok(Value::Null)
        }
        "getAttributeNS" => Ok(dom
            .attribute(
                handle(runtime, arguments, 0)?,
                &attribute_name(
                    bridge_arg(arguments, 1, "namespace")?,
                    bridge_arg(arguments, 2, "attribute name")?,
                )?,
            )
            .map_err(dom_error)?
            .map(Value::String)
            .unwrap_or(Value::Null)),
        "setAttributeNS" => {
            let node = handle(runtime, arguments, 0)?;
            let name = attribute_name(
                bridge_arg(arguments, 1, "namespace")?,
                bridge_arg(arguments, 2, "attribute name")?,
            )?;
            dom.set_attribute(node, &name, bridge_arg(arguments, 3, "attribute value")?)
                .map_err(dom_error)?;
            Ok(Value::Null)
        }
        "removeAttributeNS" => {
            let node = handle(runtime, arguments, 0)?;
            let name = attribute_name(
                bridge_arg(arguments, 1, "namespace")?,
                bridge_arg(arguments, 2, "attribute name")?,
            )?;
            dom.remove_attribute(node, &name).map_err(dom_error)?;
            Ok(Value::Null)
        }
        // Each name with the namespace it is in, which is what an attribute node
        // needs to read its own value back and what `attributeNames` cannot say.
        "attributeEntries" => Ok(json!(
            attribute_names(dom, handle(runtime, arguments, 0)?)?
                .into_iter()
                .map(|name| json!({
                    "namespace": namespace_uri(&name.namespace),
                    "name": name.local,
                }))
                .collect::<Vec<_>>()
        )),
        // Local names: an attribute is keyed by namespace and local name here,
        // so there is no prefix left to qualify one with.
        "attributeNames" => Ok(json!(
            attribute_names(dom, handle(runtime, arguments, 0)?)?
                .into_iter()
                .map(|name| name.local)
                .collect::<Vec<_>>()
        )),
        "hasAttribute" => Ok(Value::Bool(
            dom.attribute(
                handle(runtime, arguments, 0)?,
                &DomName::attribute(
                    bridge_arg(arguments, 1, "attribute name")?.to_ascii_lowercase(),
                ),
            )
            .map_err(dom_error)?
            .is_some(),
        )),
        // Form-control state, which is not the matching content attribute: see
        // `DomBackend::form_value`. Read and written through the renderer's own
        // control state so what JavaScript sees is what is painted.
        "formValue" => Ok(Value::String(
            dom.form_value(handle(runtime, arguments, 0)?)
                .map_err(dom_error)?,
        )),
        "setFormValue" => {
            let node = handle(runtime, arguments, 0)?;
            dom.set_form_value(node, bridge_arg(arguments, 1, "control value")?)
                .map_err(dom_error)?;
            Ok(Value::Null)
        }
        "formChecked" => Ok(Value::Bool(
            dom.form_checked(handle(runtime, arguments, 0)?)
                .map_err(dom_error)?,
        )),
        "setFormChecked" => {
            let node = handle(runtime, arguments, 0)?;
            let checked = bridge_arg(arguments, 1, "control checkedness")? == "true";
            dom.set_form_checked(node, checked).map_err(dom_error)?;
            Ok(Value::Null)
        }
        _ => return Ok(None),
    }?;
    Ok(Some(value))
}
