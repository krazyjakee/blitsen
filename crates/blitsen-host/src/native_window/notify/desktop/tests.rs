use super::*;

#[test]
fn update_patch_preserves_unspecified_values() {
    let mut options = NotificationOptions {
        title: "Before".into(),
        body: "Body".into(),
        app_name: Some("Demo".into()),
        timeout: Some(1000),
        urgency: "normal".into(),
        icon: None,
        actions: vec![],
    };
    options.apply(NotificationPatch {
        title: Some("After".into()),
        body: None,
        app_name: None,
        timeout: None,
        urgency: Some("critical".into()),
        icon: None,
        actions: None,
    });
    assert_eq!(options.title, "After");
    assert_eq!(options.body, "Body");
    assert_eq!(options.app_name.as_deref(), Some("Demo"));
    assert_eq!(options.timeout, Some(1000));
    assert_eq!(options.urgency, "critical");
}

#[test]
fn the_unbundled_macos_refusal_names_a_command_and_borrows_no_identity() {
    // Both halves of #253's acceptance: the limitation is actionable, and
    // the action is one the reader can type.
    assert!(NO_BUNDLE_IDENTITY.contains("blitsen --dev-bundle"));
    assert!(NO_BUNDLE_IDENTITY.contains("blitsen build --bundle-id <id> --sign <command>"));
    // And the shortcut it refuses stays refused: submitting under an
    // installed application's identifier is what the legacy backend's
    // `get_bundle_identifier_or_default` does, and no message that named one
    // could be read as anything but an invitation to do it.
    for borrowed in ["com.apple.", "Terminal", "Script Editor", "iTerm"] {
        assert!(
            !NO_BUNDLE_IDENTITY.contains(borrowed),
            "the macOS notification refusal must not name {borrowed}"
        );
    }
}

#[test]
fn the_unregistered_windows_refusal_names_the_identity_rather_than_a_verdict() {
    // What a reader has to be able to tell apart: an application Windows
    // never heard of, and one whose notifications a person switched off.
    assert!(NO_TOAST_IDENTITY.contains("AppUserModelID"));
    assert!(NO_TOAST_IDENTITY.contains("blitsen build --bundle-id <id>"));
    assert!(
        !NO_TOAST_IDENTITY.contains("denied"),
        "an unregistered identity is not a permission anybody denied"
    );
}

/// What only a Windows host can answer.
///
/// Replacement by tag and removal from notification history are behaviours of
/// the Windows notification platform rather than of anything this file could
/// stand in for, so these talk to the real platform under the same identity,
/// group and tags [`NotifyController`] uses. They need no event loop, which is
/// what lets them run under the ordinary `cargo test` the Windows job runs.
///
/// That process is also the one Windows knows least about: nothing registers an
/// AppUserModelID for a test binary, so what CI exercises is the identity-less
/// half of the contract. Both halves are asserted here, and the half that needs
/// a registered identity to mean anything skips loudly when there is none.
#[cfg(all(test, target_os = "windows"))]
mod windows_tests {
    use super::*;
    use windows::UI::Notifications::ToastNotificationManager;
    use windows::core::HSTRING;
    use winrt_toast_reborn::ToastManager;

    /// The tags Blitsen's group currently holds in notification history.
    fn tags() -> Vec<String> {
        ToastNotificationManager::History()
            .and_then(|history| history.GetHistoryWithId(&HSTRING::from(app_id())))
            .expect("notification history is readable")
            .into_iter()
            .filter(|toast| toast.Group().is_ok_and(|group| group == GROUP))
            .map(|toast| {
                toast
                    .Tag()
                    .expect("a delivered toast keeps the tag it was shown with")
                    .to_string()
            })
            .collect()
    }

    /// Asserts that notification history settles on `expected`.
    ///
    /// `Show` hands a toast to the notification platform rather than to the
    /// Action Center, and a removal is acknowledged the same way, so reading
    /// back at once races the platform instead of testing it. The wait is
    /// bounded and the assertion it ends in is the whole one.
    fn settles_on(expected: &[&str]) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut observed = tags();
        while observed != expected && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(50));
            observed = tags();
        }
        assert_eq!(observed, expected);
    }

    fn options(body: &str) -> NotificationOptions {
        NotificationOptions {
            title: "Export complete".into(),
            body: body.into(),
            app_name: None,
            timeout: Some(1000),
            urgency: "normal".into(),
            icon: None,
            actions: vec![crate::dom_bridge::notify::NotificationAction {
                id: "open".into(),
                title: "Open archive".into(),
            }],
        }
    }

    fn test_toast(public_id: &str, options: &NotificationOptions) -> winrt_toast_reborn::Toast {
        toast(public_id, options, "test-session", 1)
            .expect("the toast builds")
            .0
    }

    #[test]
    fn permission_is_the_notifier_setting_or_the_missing_identity() {
        // Both outcomes are the contract, and which one a machine gives is a
        // property of the machine: a Windows installation carrying the identity
        // has a notifier with a setting, and one that never registered it —
        // the CI runner this runs on — has no notifier at all.
        let read = NotifyController::permission(false);
        match &read {
            Ok(setting) => assert!(
                *setting == json!("granted") || *setting == json!("denied"),
                "Windows has no undetermined notification state, but reported {setting}"
            ),
            Err(refusal) => assert_eq!(
                refusal, NO_TOAST_IDENTITY,
                "an unreadable notifier must name the identity it is missing, not an HRESULT"
            ),
        }
        assert_eq!(
            NotifyController::permission(true),
            read,
            "requesting must not prompt or change what the notifier reports"
        );
    }

    #[test]
    fn a_shown_toast_is_replaced_and_removed_through_its_session_id() {
        let public_id = "n-windows-lifecycle";
        let manager = ToastManager::new(app_id());
        manager
            .remove_grouped_tag(GROUP, public_id)
            .expect("notification history is writable");

        match NotifyController::permission(false) {
            // A notifier the user or policy has switched off is Windows
            // declining to deliver, so there is nothing for history to hold and
            // delivery is not the platform's promise to keep.
            Ok(setting) => {
                let delivers = setting == json!("granted");
                manager
                    .show(&test_toast(public_id, &options("The archive is ready.")))
                    .expect("the toast is accepted");
                if delivers {
                    settles_on(&[public_id]);
                }

                manager
                    .show(&test_toast(public_id, &options("Copied to Downloads.")))
                    .expect("the replacement is accepted");
                if delivers {
                    settles_on(&[public_id]);
                }
            }
            // Replacement is a property of the notifier that accepted the first
            // toast, and a machine with no registered identity has no notifier
            // to accept one, so submitting here would test the platform's
            // refusal rather than Blitsen's tagging. Said out loud, because a
            // silent skip is how a test that measured nothing looks from CI.
            Err(refusal) => {
                assert_eq!(
                    refusal, NO_TOAST_IDENTITY,
                    "a toast that cannot be submitted must say which prerequisite is absent"
                );
                eprintln!(
                    "SKIPPED the replacement half of \
                     a_shown_toast_is_replaced_and_removed_through_its_session_id: {refusal}"
                );
            }
        }

        // Unconditional: an ID Blitsen closed must leave nothing behind whether
        // or not a toast for it was ever displayed.
        manager
            .remove_grouped_tag(GROUP, public_id)
            .expect("the toast is removable");
        settles_on(&[]);
    }

    #[test]
    fn an_unusable_option_is_rejected_before_a_toast_reaches_windows() {
        // Windows resolves a toast image through a URI, so a relative path is
        // not a path it can be asked about later.
        let mut relative_icon = options("The archive is ready.");
        relative_icon.icon = Some("archive.png".into());
        assert!(toast("n1", &relative_icon, "test-session", 1).is_err());

        let mut unknown_urgency = options("The archive is ready.");
        unknown_urgency.urgency = "urgent".into();
        assert!(toast("n1", &unknown_urgency, "test-session", 1).is_err());
    }
}
