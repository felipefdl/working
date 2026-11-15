//! Windows: the release build is a GUI program, which Windows starts without a console, so its log (stderr) would
//! go nowhere. Started from a terminal, it writes to that terminal's console instead; started from the Start Menu,
//! it has none to attach to and logs nothing, as before. A redirected stderr is left as it is.
#![allow(unsafe_code, reason = "console FFI")]

use windows_sys::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole, GetStdHandle, STD_ERROR_HANDLE};

/// Attaches to the console of the process that started this one when stderr has nowhere to go.
pub fn attach() {
  // SAFETY: reads the process's standard error handle; no pointers involved.
  let stderr = unsafe { GetStdHandle(STD_ERROR_HANDLE) };
  // A console program, or a GUI one with stderr redirected, already has somewhere to write.
  if !stderr.is_null() && stderr != windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
    return;
  }
  // SAFETY: no arguments to uphold; it fails harmlessly when the parent has no console.
  unsafe { AttachConsole(ATTACH_PARENT_PROCESS) };
}
