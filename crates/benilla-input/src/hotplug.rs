//! How gilrs learns that a pad left or came back. gilrs-core listens to udev for that, and a udev
//! event never reaches a container (a Steam Deck's distrobox), so a pad whose node vanishes (Steam
//! restarting, a SteamOS update) is never reported gone: gilrs' force-feedback thread keeps writing
//! to the dead node and logs ENODEV every 50 ms, and a pad that returns is never seen. gilrs-core
//! has a second way, a watch on `/dev/input` ([`GILRS_DISABLE_UDEV`]), which a container does see;
//! it reports the removal, so gilrs closes the node's force-feedback device, and reports the
//! return.

use std::path::Path;

/// The variable gilrs-core reads to watch `/dev/input` instead of udev.
const GILRS_DISABLE_UDEV: &str = "GILRS_DISABLE_UDEV";

/// Files a container runtime creates in every container it starts (podman, docker).
const CONTAINER_MARKERS: [&str; 2] = ["/run/.containerenv", "/.dockerenv"];

/// Whether gilrs should watch `/dev/input`: in a container, unless the launcher already chose.
fn watch_dev_input(chosen: bool, in_container: bool) -> bool {
    !chosen && in_container
}

/// Before gilrs is built: have it watch `/dev/input` when this process runs in a container.
pub(crate) fn watch_dev_input_in_container() {
    let in_container = CONTAINER_MARKERS.iter().any(|p| Path::new(p).exists())
        || std::env::var_os("CONTAINER_ID").is_some();
    if watch_dev_input(std::env::var_os(GILRS_DISABLE_UDEV).is_some(), in_container) {
        std::env::set_var(GILRS_DISABLE_UDEV, "1");
    }
}

#[cfg(test)]
mod tests {
    use super::watch_dev_input;

    #[test]
    fn a_container_watches_dev_input_unless_the_launcher_chose() {
        assert!(watch_dev_input(false, true));
        assert!(!watch_dev_input(true, true), "the launcher's choice stands");
        assert!(!watch_dev_input(false, false), "a host keeps udev");
    }
}
