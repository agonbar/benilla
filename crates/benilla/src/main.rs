//! The `benilla` launcher: a shim that carries the build stamp, so a new commit recompiles these
//! few lines and relinks instead of rebuilding `benilla-app`.

use benilla_app::BuildId;

fn main() -> benilla_app::AppExit {
    // With `--features vr` the client starts in the headset through OpenXR, and flat without one;
    // with `--features pad` (which `vr` implies) it also reads a gamepad.
    #[cfg(feature = "vr")]
    let run = |build| {
        benilla_vr::run_with(build, benilla_input::enable_gilrs, |app| {
            app.add_plugins(benilla_input::PadPlugin);
        })
    };
    #[cfg(all(feature = "pad", not(feature = "vr")))]
    let run = benilla_input::run;
    #[cfg(not(feature = "pad"))]
    let run = benilla_app::run;
    run(BuildId {
        version: env!("CARGO_PKG_VERSION"),
        describe: env!("BENILLA_GIT_DESCRIBE"),
        sha: env!("BENILLA_GIT_SHA"),
        short: env!("BENILLA_GIT_SHORT"),
        date: env!("BENILLA_GIT_DATE"),
        profile: env!("BENILLA_PROFILE"),
        project_dir: env!("BENILLA_PROJECT_DIR"),
        ..Default::default()
    })
}
