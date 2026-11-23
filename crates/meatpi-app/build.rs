fn main() {
    // The app's own palette (ui/theme.slint) is always dark, so pin the
    // built-in widgets (LineEdit, CheckBox, Button) to the dark variant too.
    // Otherwise they follow the system theme and render dark-on-dark labels
    // whenever the phone or browser is in light mode.
    let config = slint_build::CompilerConfiguration::new().with_style("fluent-dark".into());
    slint_build::compile_with_config("ui/app.slint", config).expect("failed to compile Slint UI");
}
