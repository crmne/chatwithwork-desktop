// Runs before styles and before any asynchronous native command. The child
// must paint the authenticated shell's theme even while app_info is pending.
if (window.CWW_SETTINGS_PANE) {
  document.documentElement.classList.add("embedded");
  document.documentElement.style.colorScheme = window.CWW_SETTINGS_PANE.theme;
}
