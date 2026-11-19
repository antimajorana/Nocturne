//! iOS / iPadOS Keychain secret storage implementing [`nocturne_engine::SecretStore`].
//!
//! Stores the user's Infomaniak Personal API Token in the device-only Keychain
//! (`kSecClassGenericPassword`, `kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly`) so the token
//! never touches `settings.toml` or `Documents/Nocturne`.

pub const KEYCHAIN_SERVICE: &str = "io.github.antimajorana.nocturne.kdrive";

/// Normalizes a secret account key for Keychain lookup.
#[must_use]
pub fn normalize_account_key(key: &str) -> String {
    let trimmed = key.trim();
    if trimmed.is_empty() { "default".to_string() } else { trimmed.to_string() }
}

#[cfg(target_os = "ios")]
mod imp {
    use super::{KEYCHAIN_SERVICE, normalize_account_key};
    use core::ffi::c_void;
    use nocturne_engine::{SecretStore, SyncError};
    use objc2::MainThreadMarker;
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2_foundation::{NSData, NSDictionary, NSString};

    type OSStatus = i32;
    type CFDictionaryRef = *const c_void;
    type CFTypeRef = *const c_void;

    const ERR_SEC_SUCCESS: OSStatus = 0;
    const ERR_SEC_ITEM_NOT_FOUND: OSStatus = -25300;
    const ERR_SEC_DUPLICATE_ITEM: OSStatus = -25299;

    #[link(name = "Security", kind = "framework")]
    // SAFETY: Apple Security.framework C declarations matching the iOS SDK ABI.
    #[allow(unsafe_code)]
    unsafe extern "C" {
        static kSecClass: CFTypeRef;
        static kSecClassGenericPassword: CFTypeRef;
        static kSecAttrService: CFTypeRef;
        static kSecAttrAccount: CFTypeRef;
        static kSecAttrAccessible: CFTypeRef;
        static kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly: CFTypeRef;
        static kSecValueData: CFTypeRef;
        static kSecReturnData: CFTypeRef;
        static kSecMatchLimit: CFTypeRef;
        static kSecMatchLimitOne: CFTypeRef;

        fn SecItemAdd(attributes: CFDictionaryRef, result: *mut CFTypeRef) -> OSStatus;
        fn SecItemCopyMatching(query: CFDictionaryRef, result: *mut CFTypeRef) -> OSStatus;
        fn SecItemUpdate(query: CFDictionaryRef, attributes_to_update: CFDictionaryRef) -> OSStatus;
        fn SecItemDelete(query: CFDictionaryRef) -> OSStatus;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    // SAFETY: Apple CoreFoundation.framework C declarations matching the iOS SDK ABI.
    #[allow(unsafe_code)]
    unsafe extern "C" {
        static kCFBooleanTrue: CFTypeRef;
        fn CFRelease(cf: CFTypeRef);
    }

    #[allow(unsafe_code)]
    fn cf_key(raw: CFTypeRef) -> &'static NSString {
        // SAFETY: Security.framework `kSec*` keys are immortal static `CFStringRef` constants,
        // which are toll-free bridged with `NSString` on iOS.
        unsafe { &*(raw.cast::<NSString>()) }
    }

    #[allow(unsafe_code)]
    fn cf_obj(raw: CFTypeRef) -> &'static AnyObject {
        // SAFETY: Security/CoreFoundation `kSec*` / `kCFBooleanTrue` values are immortal static
        // CoreFoundation objects toll-free bridged with Objective-C `id` (`AnyObject`).
        unsafe { &*(raw.cast::<AnyObject>()) }
    }

    fn base_query(account: &str) -> Retained<NSDictionary<NSString, AnyObject>> {
        let service = NSString::from_str(KEYCHAIN_SERVICE);
        let acct = NSString::from_str(&normalize_account_key(account));
        #[allow(unsafe_code)]
        // SAFETY: Accessing static Security.framework constants linked from Security.framework.
        let (k_class, v_generic, k_service, k_account) =
            unsafe { (cf_key(kSecClass), cf_obj(kSecClassGenericPassword), cf_key(kSecAttrService), cf_key(kSecAttrAccount)) };
        let service_obj: &AnyObject = service.as_ref();
        let acct_obj: &AnyObject = acct.as_ref();
        NSDictionary::from_slices(&[k_class, k_service, k_account], &[v_generic, service_obj, acct_obj])
    }

    /// iPadOS / iOS Keychain store initialized on the UIKit main thread for cloud sync credentials.
    pub struct IosKeychainStore {
        _private: (),
    }

    impl IosKeychainStore {
        #[must_use]
        pub fn new(_mtm: MainThreadMarker) -> Self {
            Self { _private: () }
        }
    }

    impl SecretStore for IosKeychainStore {
        fn get_secret(&self, key: &str) -> Result<Option<String>, SyncError> {
            let service = NSString::from_str(KEYCHAIN_SERVICE);
            let acct = NSString::from_str(&normalize_account_key(key));
            #[allow(unsafe_code)]
            // SAFETY: Accessing static Security/CoreFoundation constants.
            let (k_class, v_generic, k_service, k_account, k_ret, v_true, k_limit, v_one) = unsafe {
                (
                    cf_key(kSecClass),
                    cf_obj(kSecClassGenericPassword),
                    cf_key(kSecAttrService),
                    cf_key(kSecAttrAccount),
                    cf_key(kSecReturnData),
                    cf_obj(kCFBooleanTrue),
                    cf_key(kSecMatchLimit),
                    cf_obj(kSecMatchLimitOne),
                )
            };
            let service_obj: &AnyObject = service.as_ref();
            let acct_obj: &AnyObject = acct.as_ref();
            let query: Retained<NSDictionary<NSString, AnyObject>> =
                NSDictionary::from_slices(&[k_class, k_service, k_account, k_ret, k_limit], &[v_generic, service_obj, acct_obj, v_true, v_one]);

            let mut out: CFTypeRef = core::ptr::null();
            #[allow(unsafe_code)]
            // SAFETY: `query` is a valid toll-free bridged `NSDictionary` (`CFDictionaryRef`), and
            // `&mut out` receives a +1 retained `CFDataRef` on `ERR_SEC_SUCCESS`.
            let status = unsafe { SecItemCopyMatching(Retained::as_ptr(&query).cast(), &mut out) };
            match status {
                ERR_SEC_SUCCESS => {
                    if out.is_null() {
                        return Ok(None);
                    }
                    #[allow(unsafe_code)]
                    // SAFETY: `SecItemCopyMatching` with `kSecReturnData = kCFBooleanTrue` returns a
                    // +1 retained `CFDataRef` (toll-free bridged with `NSData`), which we copy and release.
                    let bytes = unsafe {
                        let ns_data = &*(out.cast::<NSData>());
                        let copied = ns_data.to_vec();
                        CFRelease(out);
                        copied
                    };
                    let text = String::from_utf8(bytes).map_err(|_| SyncError::Io("stored Keychain token is not valid UTF-8".into()))?;
                    let trimmed = text.trim().to_string();
                    if trimmed.is_empty() { Ok(None) } else { Ok(Some(trimmed)) }
                }
                ERR_SEC_ITEM_NOT_FOUND => Ok(None),
                code => Err(SyncError::Io(format!("iOS Keychain read failed (OSStatus {code})"))),
            }
        }

        fn set_secret(&self, key: &str, secret: &str) -> Result<(), SyncError> {
            let trimmed = secret.trim();
            if trimmed.is_empty() {
                return self.delete_secret(key);
            }
            let service = NSString::from_str(KEYCHAIN_SERVICE);
            let acct = NSString::from_str(&normalize_account_key(key));
            let val_data = NSData::with_bytes(trimmed.as_bytes());
            #[allow(unsafe_code)]
            // SAFETY: Accessing static Security.framework constants.
            let (k_class, v_generic, k_service, k_account, k_access, v_access, k_val) = unsafe {
                (
                    cf_key(kSecClass),
                    cf_obj(kSecClassGenericPassword),
                    cf_key(kSecAttrService),
                    cf_key(kSecAttrAccount),
                    cf_key(kSecAttrAccessible),
                    cf_obj(kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly),
                    cf_key(kSecValueData),
                )
            };
            let service_obj: &AnyObject = service.as_ref();
            let acct_obj: &AnyObject = acct.as_ref();
            let val_obj: &AnyObject = val_data.as_ref();
            let attrs: Retained<NSDictionary<NSString, AnyObject>> =
                NSDictionary::from_slices(&[k_class, k_service, k_account, k_access, k_val], &[v_generic, service_obj, acct_obj, v_access, val_obj]);

            #[allow(unsafe_code)]
            // SAFETY: `attrs` is a valid toll-free bridged `CFDictionaryRef`; `result` is null.
            let status = unsafe { SecItemAdd(Retained::as_ptr(&attrs).cast(), core::ptr::null_mut()) };
            match status {
                ERR_SEC_SUCCESS => Ok(()),
                ERR_SEC_DUPLICATE_ITEM => {
                    let query = base_query(key);
                    let update: Retained<NSDictionary<NSString, AnyObject>> = NSDictionary::from_slices(&[k_val, k_access], &[val_obj, v_access]);
                    #[allow(unsafe_code)]
                    // SAFETY: Both `query` and `update` are valid toll-free bridged `CFDictionaryRef`s.
                    let upd_status = unsafe { SecItemUpdate(Retained::as_ptr(&query).cast(), Retained::as_ptr(&update).cast()) };
                    if upd_status == ERR_SEC_SUCCESS {
                        Ok(())
                    } else {
                        Err(SyncError::Io(format!("iOS Keychain update failed (OSStatus {upd_status})")))
                    }
                }
                code => Err(SyncError::Io(format!("iOS Keychain save failed (OSStatus {code})"))),
            }
        }

        fn delete_secret(&self, key: &str) -> Result<(), SyncError> {
            let query = base_query(key);
            #[allow(unsafe_code)]
            // SAFETY: `query` is a valid toll-free bridged `CFDictionaryRef`.
            let status = unsafe { SecItemDelete(Retained::as_ptr(&query).cast()) };
            match status {
                ERR_SEC_SUCCESS | ERR_SEC_ITEM_NOT_FOUND => Ok(()),
                code => Err(SyncError::Io(format!("iOS Keychain delete failed (OSStatus {code})"))),
            }
        }
    }
}

#[cfg(target_os = "ios")]
pub use imp::IosKeychainStore;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_account_keys() {
        assert_eq!(normalize_account_key("  kdrive_personal_token "), "kdrive_personal_token");
        assert_eq!(normalize_account_key("   "), "default");
    }
}
