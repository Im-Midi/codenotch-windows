//! Keys typed into the API-keys window live in Windows Credential Manager (generic credentials,
//! persisted for this user on this machine), never in config.json: that file is plain text in
//! %APPDATA% and gets copied around with logs when someone asks for help.

pub const COMMANDCODE: &str = "codenotch:commandcode";
pub const ROUTER9_TOKEN: &str = "codenotch:router9-token";
/// Not a secret, but kept beside its token rather than in config.json: that file is rewritten whole
/// from each process's in-memory copy (drag, tray toggles, language), and a URL saved from the
/// API-keys window was found written back over with a stale null while its token survived here.
pub const ROUTER9_URL: &str = "codenotch:router9-url";
/// Cloudflare Access service token, for a 9Router published behind Access (Zero Trust)
pub const ROUTER9_CF_ID: &str = "codenotch:router9-cf-access-id";
pub const ROUTER9_CF_SECRET: &str = "codenotch:router9-cf-access-secret";

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(windows)]
pub fn get(target: &str) -> Option<String> {
    use windows::core::PCWSTR;
    use windows::Win32::Security::Credentials::{CredFree, CredReadW, CREDENTIALW, CRED_TYPE_GENERIC};
    let t = wide(target);
    let mut pcred: *mut CREDENTIALW = std::ptr::null_mut();
    unsafe {
        if CredReadW(PCWSTR(t.as_ptr()), CRED_TYPE_GENERIC, 0, &mut pcred).is_err() || pcred.is_null() {
            return None;
        }
        let c = &*pcred;
        let blob = if c.CredentialBlobSize > 0 && !c.CredentialBlob.is_null() {
            std::slice::from_raw_parts(c.CredentialBlob, c.CredentialBlobSize as usize).to_vec()
        } else {
            Vec::new()
        };
        CredFree(pcred as *const core::ffi::c_void);
        String::from_utf8(blob).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
    }
}

#[cfg(windows)]
pub fn set(target: &str, value: &str) -> Result<(), String> {
    use windows::core::PWSTR;
    use windows::Win32::Security::Credentials::{CredWriteW, CREDENTIALW, CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC};
    let mut t = wide(target);
    let mut user = wide("codenotch");
    let mut blob = value.as_bytes().to_vec();
    let cred = CREDENTIALW {
        Type: CRED_TYPE_GENERIC,
        TargetName: PWSTR(t.as_mut_ptr()),
        CredentialBlobSize: blob.len() as u32,
        CredentialBlob: blob.as_mut_ptr(),
        Persist: CRED_PERSIST_LOCAL_MACHINE,
        UserName: PWSTR(user.as_mut_ptr()),
        ..Default::default()
    };
    unsafe { CredWriteW(&cred, 0) }.map_err(|e| e.message().to_string())
}

#[cfg(windows)]
pub fn delete(target: &str) {
    use windows::core::PCWSTR;
    use windows::Win32::Security::Credentials::{CredDeleteW, CRED_TYPE_GENERIC};
    let t = wide(target);
    let _ = unsafe { CredDeleteW(PCWSTR(t.as_ptr()), CRED_TYPE_GENERIC, 0) };
}

#[cfg(not(windows))]
pub fn get(_target: &str) -> Option<String> {
    None
}
#[cfg(not(windows))]
pub fn set(_target: &str, _value: &str) -> Result<(), String> {
    Err("credential storage is only implemented on Windows".into())
}
#[cfg(not(windows))]
pub fn delete(_target: &str) {}

#[cfg(all(test, windows))]
mod tests {
    #[test]
    #[ignore = "writes to the real Windows Credential Manager"]
    fn round_trip() {
        const T: &str = "codenotch:selftest";
        super::set(T, "sëcret-✓").unwrap();
        assert_eq!(super::get(T).as_deref(), Some("sëcret-✓"));
        super::delete(T);
        assert_eq!(super::get(T), None);
    }
}
