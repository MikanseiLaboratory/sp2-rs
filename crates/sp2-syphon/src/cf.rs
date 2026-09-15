//! Core Foundation / Foundation helpers: strings, keyed archiving, UUIDs and
//! the server description dictionary.

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::ClassType;
use objc2_core_foundation::{CFRetained, CFString, CFUUID};
use objc2_foundation::{
    NSArray, NSData, NSDictionary, NSKeyedArchiver, NSKeyedUnarchiver, NSNumber, NSProcessInfo,
    NSSet, NSString,
};
use sp2_core::{Error, Result};

use crate::constants::{
    uuid_string, KEY_APP_NAME, KEY_DICTIONARY_VERSION, KEY_NAME, KEY_SURFACES, KEY_SURFACE_TYPE,
    KEY_UUID,
};
use crate::description::ServerDescription;

/// Wrapper asserting that a Core Foundation object may cross threads.
///
/// Used for `CFMessagePort` and `CFRunLoop`, which Apple documents as thread
/// safe, but which `objc2-core-foundation` conservatively leaves `!Send`.
pub(crate) struct SendCf<T>(pub CFRetained<T>);

// SAFETY: only instantiated with types documented as thread safe (see above);
// callers never rely on interior mutability across threads beyond what the
// CF API itself guarantees.
unsafe impl<T> Send for SendCf<T> {}
unsafe impl<T> Sync for SendCf<T> {}

/// Create a `CFString`.
pub(crate) fn cf_string(s: &str) -> CFRetained<CFString> {
    CFString::from_str(s)
}

/// Create a new `info.v002.Syphon.<UUID>` identifier.
pub(crate) fn new_uuid_string() -> Result<String> {
    let uuid = CFUUID::new(None).ok_or_else(|| Error::backend("CFUUIDCreate failed"))?;
    let string = CFUUID::new_string(None, Some(&uuid))
        .ok_or_else(|| Error::backend("CFUUIDCreateString failed"))?;
    Ok(uuid_string(&string.to_string()))
}

/// Name of the current process (`NSProcessInfo.processName`).
pub(crate) fn process_name() -> String {
    NSProcessInfo::processInfo().processName().to_string()
}

/// Payload of a `CFMessagePort` message after keyed unarchiving.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Payload {
    /// No data.
    None,
    /// An `NSString`.
    Text(String),
    /// An `NSNumber`.
    Number(i64),
}

impl Payload {
    /// Text payload, if any.
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Payload::Text(s) => Some(s),
            _ => None,
        }
    }

    /// Numeric payload, if any.
    pub fn as_number(&self) -> Option<i64> {
        match self {
            Payload::Number(n) => Some(*n),
            _ => None,
        }
    }
}

/// Archive a payload with `NSKeyedArchiver` (secure coding), like the
/// reference implementation.
pub(crate) fn archive(payload: &Payload) -> Result<Option<Vec<u8>>> {
    let (text, number);
    let object: &AnyObject = match payload {
        Payload::None => return Ok(None),
        Payload::Text(s) => {
            text = NSString::from_str(s);
            &text
        }
        Payload::Number(n) => {
            number = if *n >= 0 && *n <= u32::MAX as i64 {
                NSNumber::new_u32(*n as u32)
            } else {
                NSNumber::new_i64(*n)
            };
            &number
        }
    };
    // SAFETY: `object` is a valid NSString / NSNumber supporting secure coding.
    let data = unsafe {
        NSKeyedArchiver::archivedDataWithRootObject_requiringSecureCoding_error(object, true)
    }
    .map_err(|e| Error::backend(format!("NSKeyedArchiver failed: {e}")))?;
    Ok(Some(data.to_vec()))
}

/// Decode a keyed archive containing an `NSString` or `NSNumber`.
pub(crate) fn unarchive(bytes: &[u8]) -> Payload {
    if bytes.is_empty() {
        return Payload::None;
    }
    let data = NSData::with_bytes(bytes);
    let classes = NSSet::from_slice(&[NSString::class(), NSNumber::class()]);
    // SAFETY: `classes` and `data` are valid objects.
    let object = match unsafe {
        NSKeyedUnarchiver::unarchivedObjectOfClasses_fromData_error(&classes, &data)
    } {
        Ok(object) => object,
        Err(e) => {
            log::debug!("NSKeyedUnarchiver failed: {e}");
            return Payload::None;
        }
    };
    if let Some(s) = object.downcast_ref::<NSString>() {
        return Payload::Text(s.to_string());
    }
    if let Some(n) = object.downcast_ref::<NSNumber>() {
        return Payload::Number(n.as_i64());
    }
    Payload::None
}

/// Build the `userInfo` dictionary for server notifications.
pub(crate) fn description_to_dictionary(desc: &ServerDescription) -> Retained<NSDictionary> {
    let surfaces: Vec<Retained<NSDictionary>> = desc
        .surfaces
        .iter()
        .map(|surface| {
            let key = NSString::from_str(KEY_SURFACE_TYPE);
            let value = NSString::from_str(surface);
            let value: &AnyObject = &value;
            let dict = NSDictionary::<NSString, AnyObject>::from_slices(&[&*key], &[value]);
            // SAFETY: erasing the generic parameters does not change the object.
            unsafe { Retained::cast_unchecked::<NSDictionary>(dict) }
        })
        .collect();
    let surfaces_array = NSArray::from_retained_slice(&surfaces);

    let keys = [
        NSString::from_str(KEY_DICTIONARY_VERSION),
        NSString::from_str(KEY_UUID),
        NSString::from_str(KEY_NAME),
        NSString::from_str(KEY_APP_NAME),
        NSString::from_str(KEY_SURFACES),
    ];
    let version = NSNumber::new_u32(desc.version);
    let uuid = NSString::from_str(&desc.uuid);
    let name = NSString::from_str(&desc.name);
    let app_name = NSString::from_str(&desc.app_name);
    let values: [&AnyObject; 5] = [&version, &uuid, &name, &app_name, &surfaces_array];
    let key_refs: [&NSString; 5] = [&keys[0], &keys[1], &keys[2], &keys[3], &keys[4]];
    let dict = NSDictionary::<NSString, AnyObject>::from_slices(&key_refs, &values);
    // SAFETY: erasing the generic parameters does not change the object.
    unsafe { Retained::cast_unchecked::<NSDictionary>(dict) }
}

/// Parse a notification `userInfo` dictionary into a [`ServerDescription`].
///
/// Returns `None` when the UUID key is missing.
pub(crate) fn dictionary_to_description(dict: &NSDictionary) -> Option<ServerDescription> {
    let get = |key: &str| -> Option<Retained<AnyObject>> {
        let key = NSString::from_str(key);
        let key: &AnyObject = &key;
        dict.objectForKey(key)
    };
    let string = |key: &str| -> Option<String> {
        get(key).and_then(|obj| obj.downcast_ref::<NSString>().map(|s| s.to_string()))
    };

    let uuid = string(KEY_UUID)?;
    let version = get(KEY_DICTIONARY_VERSION)
        .and_then(|obj| obj.downcast_ref::<NSNumber>().map(|n| n.as_u32()))
        .unwrap_or(0);
    let name = string(KEY_NAME).unwrap_or_default();
    let app_name = string(KEY_APP_NAME).unwrap_or_default();

    let mut surfaces = Vec::new();
    if let Some(array) = get(KEY_SURFACES) {
        if let Some(array) = array.downcast_ref::<NSArray>() {
            let type_key = NSString::from_str(KEY_SURFACE_TYPE);
            let type_key: &AnyObject = &type_key;
            for item in array.iter() {
                if let Some(surface) = item.downcast_ref::<NSDictionary>() {
                    if let Some(value) = surface.objectForKey(type_key) {
                        if let Some(value) = value.downcast_ref::<NSString>() {
                            surfaces.push(value.to_string());
                        }
                    }
                }
            }
        }
    }

    Some(ServerDescription {
        version,
        uuid,
        name,
        app_name,
        surfaces,
    })
}
