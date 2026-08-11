//! Explorer-facing COM entry point.
//!
//! A legacy `command` verb is launched once for every selected file, and
//! therefore cannot reliably know where one selection ends.  `IExecuteCommand`
//! receives the complete `IShellItemArray` for one invocation instead.  Keep
//! this DLL deliberately tiny: it only converts that array to file-system
//! paths and starts the normal overlay process.

use std::{
    ffi::c_void,
    path::PathBuf,
    process::Command,
    ptr::null_mut,
    sync::{
        atomic::{AtomicIsize, Ordering},
        Mutex,
    },
};

use windows::{
    core::{implement, Error, IUnknown, Interface, Result, BOOL, GUID, HRESULT, PCWSTR},
    Win32::{
        Foundation::{E_NOINTERFACE, E_POINTER, HINSTANCE, HMODULE, POINT},
        System::{
            Com::{IClassFactory, IClassFactory_Impl},
            LibraryLoader::GetModuleFileNameW,
        },
        UI::Shell::{
            IExecuteCommand, IExecuteCommand_Impl, IObjectWithSelection, IObjectWithSelection_Impl,
            IShellItemArray, SIGDN_FILESYSPATH,
        },
    },
};

/// Stable COM class registered by the installer under `DelegateExecute`.
pub const CLSID_MONSTER_DELETER_COMMAND: GUID =
    GUID::from_u128(0x8c6932f1_4c2f_4f87_9f78_b563bc6df3b1);

const CLASS_E_CLASSNOTAVAILABLE: HRESULT = HRESULT(0x8004_0111_u32 as i32);
const CLASS_E_NOAGGREGATION: HRESULT = HRESULT(0x8004_0110_u32 as i32);
const DLL_PROCESS_ATTACH: u32 = 1;

static MODULE_HANDLE: AtomicIsize = AtomicIsize::new(0);

/// Capture the DLL module handle without doing loader-lock-unsafe work.
#[no_mangle]
pub extern "system" fn DllMain(module: HINSTANCE, reason: u32, _reserved: *mut c_void) -> BOOL {
    if reason == DLL_PROCESS_ATTACH {
        MODULE_HANDLE.store(module.0 as isize, Ordering::Relaxed);
    }
    BOOL(1)
}

#[implement(IExecuteCommand, IObjectWithSelection)]
struct MonsterDeleterCommand {
    selection: Mutex<Option<IShellItemArray>>,
}

impl MonsterDeleterCommand {
    fn new() -> Self {
        Self {
            selection: Mutex::new(None),
        }
    }
}

impl IExecuteCommand_Impl for MonsterDeleterCommand_Impl {
    fn SetKeyState(&self, _grfkeystate: u32) -> Result<()> {
        Ok(())
    }

    fn SetParameters(&self, _pszparameters: &PCWSTR) -> Result<()> {
        Ok(())
    }

    fn SetPosition(&self, _pt: &POINT) -> Result<()> {
        Ok(())
    }

    fn SetShowWindow(&self, _nshow: i32) -> Result<()> {
        Ok(())
    }

    fn SetNoShowUI(&self, _fnoshowui: BOOL) -> Result<()> {
        Ok(())
    }

    fn SetDirectory(&self, _pszdirectory: &PCWSTR) -> Result<()> {
        Ok(())
    }

    fn Execute(&self) -> Result<()> {
        let selection = self
            .selection
            .lock()
            .map_err(|_| Error::from_hresult(E_NOINTERFACE))?
            .clone()
            .ok_or_else(|| Error::from_hresult(E_POINTER))?;
        let targets = selection_paths(&selection)?;
        if targets.is_empty() {
            return Ok(());
        }

        let executable = overlay_executable().ok_or_else(|| Error::from_hresult(E_NOINTERFACE))?;
        Command::new(executable)
            .arg("--run-selection")
            .args(targets)
            .spawn()
            .map_err(|_| Error::from_hresult(E_NOINTERFACE))?;
        Ok(())
    }
}

impl IObjectWithSelection_Impl for MonsterDeleterCommand_Impl {
    fn SetSelection(&self, selection: windows::core::Ref<IShellItemArray>) -> Result<()> {
        let mut stored = self
            .selection
            .lock()
            .map_err(|_| Error::from_hresult(E_NOINTERFACE))?;
        *stored = selection.cloned();
        Ok(())
    }

    fn GetSelection(&self, riid: *const GUID, output: *mut *mut c_void) -> Result<()> {
        if output.is_null() || riid.is_null() {
            return Err(Error::from_hresult(E_POINTER));
        }
        unsafe { *output = null_mut() };
        let selection = self
            .selection
            .lock()
            .map_err(|_| Error::from_hresult(E_NOINTERFACE))?;
        let Some(selection) = selection.as_ref() else {
            return Err(Error::from_hresult(E_NOINTERFACE));
        };
        unsafe {
            (Interface::vtable(selection).base__.QueryInterface)(
                Interface::as_raw(selection),
                riid,
                output,
            )
            .ok()
        }
    }
}

#[implement(IClassFactory)]
struct MonsterDeleterClassFactory;

impl IClassFactory_Impl for MonsterDeleterClassFactory_Impl {
    fn CreateInstance(
        &self,
        outer: windows::core::Ref<IUnknown>,
        riid: *const GUID,
        output: *mut *mut c_void,
    ) -> Result<()> {
        if output.is_null() || riid.is_null() {
            return Err(Error::from_hresult(E_POINTER));
        }
        if outer.is_some() {
            return Err(Error::from_hresult(CLASS_E_NOAGGREGATION));
        }
        unsafe { *output = null_mut() };
        let unknown: IUnknown = MonsterDeleterCommand::new().into();
        unsafe {
            (Interface::vtable(&unknown).QueryInterface)(Interface::as_raw(&unknown), riid, output)
                .ok()
        }
    }

    fn LockServer(&self, _lock: BOOL) -> Result<()> {
        Ok(())
    }
}

/// Standard in-proc COM activation export used by Explorer.
#[no_mangle]
pub extern "system" fn DllGetClassObject(
    class_id: *const GUID,
    riid: *const GUID,
    output: *mut *mut c_void,
) -> HRESULT {
    if class_id.is_null() || riid.is_null() || output.is_null() {
        return E_POINTER;
    }
    unsafe { *output = null_mut() };
    if unsafe { *class_id } != CLSID_MONSTER_DELETER_COMMAND {
        return CLASS_E_CLASSNOTAVAILABLE;
    }
    let unknown: IUnknown = MonsterDeleterClassFactory.into();
    unsafe {
        (Interface::vtable(&unknown).QueryInterface)(Interface::as_raw(&unknown), riid, output)
    }
}

/// The command objects are reference-counted and short-lived, so Explorer can
/// unload the DLL whenever it chooses after a call returns.
#[no_mangle]
pub extern "system" fn DllCanUnloadNow() -> HRESULT {
    HRESULT(0)
}

fn selection_paths(selection: &IShellItemArray) -> Result<Vec<PathBuf>> {
    let count = unsafe { selection.GetCount()? };
    let mut targets = Vec::with_capacity(count as usize);
    for index in 0..count {
        let item = unsafe { selection.GetItemAt(index)? };
        let name = unsafe { item.GetDisplayName(SIGDN_FILESYSPATH)? };
        let path = unsafe { name.to_string()? };
        unsafe { windows::Win32::System::Com::CoTaskMemFree(Some(name.0 as _)) };
        let path = PathBuf::from(path);
        if path.exists() {
            targets.push(path);
        }
    }
    Ok(targets)
}

fn overlay_executable() -> Option<PathBuf> {
    let module = HMODULE(MODULE_HANDLE.load(Ordering::Relaxed) as _);
    if module.0.is_null() {
        return None;
    }
    let mut buffer = vec![0_u16; 32_768];
    let length = unsafe { GetModuleFileNameW(Some(module), &mut buffer) } as usize;
    if length == 0 || length >= buffer.len() {
        return None;
    }
    let dll_path = PathBuf::from(String::from_utf16_lossy(&buffer[..length]));
    dll_path
        .parent()
        .map(|directory| directory.join("monster-deleter.exe"))
}
