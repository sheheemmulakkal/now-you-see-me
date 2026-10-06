# Patched ksni 0.3.6

Upstream: https://crates.io/crates/ksni (Unlicense, see UNLICENSE).

Local change for Now You See Me: `Tray::label()` / `Tray::label_guide()`
exposed as the Ayatana StatusNotifierItem extension properties
`XAyatanaLabel` / `XAyatanaLabelGuide` with the `XAyatanaNewLabel` signal, so
Ubuntu's AppIndicator host shows text next to the icon. Files touched:
`src/lib.rs`, `src/service.rs`, `src/dbus_interface.rs`. Everything else is
unchanged from the crates.io release. To update: copy the new release over
this directory and re-apply those three edits.
