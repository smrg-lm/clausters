//! Bounded controller/component messages; allocated on the administrative plane.
#![allow(non_snake_case)] // External SDK interface spelling.
use super::*;
use ::vst3::Steinberg::Vst::IAttributeList_::AttrID;
use std::{
    collections::HashMap,
    ffi::{CStr, CString},
};
struct Message {
    id: RefCell<CString>,
    attributes: ComWrapper<Attributes>,
}
impl Class for Message {
    type Interfaces = (IMessage,);
}
impl IMessageTrait for Message {
    unsafe fn getMessageID(&self) -> FIDString {
        self.id.borrow().as_ptr()
    }
    unsafe fn setMessageID(&self, id: FIDString) {
        if let Some(key) = unsafe { key(id) } {
            *self.id.borrow_mut() = CString::new(key).unwrap();
        }
    }
    unsafe fn getAttributes(&self) -> *mut IAttributeList {
        self.attributes
            .as_com_ref::<IAttributeList>()
            .unwrap()
            .as_ptr()
    }
}
enum Value {
    Integer(i64),
    Float(f64),
    String(Vec<u16>),
    Binary(Vec<u8>),
}
impl Value {
    fn size(&self) -> usize {
        match self {
            Self::Integer(_) | Self::Float(_) => 8,
            Self::String(v) => v.len() * 2,
            Self::Binary(v) => v.len(),
        }
    }
}
struct Attributes {
    values: RefCell<HashMap<Vec<u8>, Value>>,
}
impl Class for Attributes {
    type Interfaces = (IAttributeList,);
}
unsafe fn key(id: *const i8) -> Option<Vec<u8>> {
    if id.is_null() {
        return None;
    }
    // SAFETY: native caller supplies an SDK null-terminated identifier.
    let bytes = unsafe { CStr::from_ptr(id) }.to_bytes();
    if bytes.len() > 256 {
        None
    } else {
        Some(bytes.to_vec())
    }
}
impl Attributes {
    fn new() -> Self {
        Self {
            values: RefCell::new(HashMap::new()),
        }
    }
    unsafe fn set(&self, id: AttrID, value: Value) -> tresult {
        let Some(key) = (unsafe { key(id) }) else {
            return kInvalidArgument;
        };
        let mut values = self.values.borrow_mut();
        let previous = values.get(&key).map_or(0, Value::size);
        if (values.len() == 64 && !values.contains_key(&key))
            || values.values().map(Value::size).sum::<usize>() - previous + value.size()
                > MAX_STATE_BYTES
        {
            return kResultFalse;
        }
        values.insert(key, value);
        kResultOk
    }
}
impl IAttributeListTrait for Attributes {
    unsafe fn setInt(&self, id: AttrID, value: i64) -> tresult {
        unsafe { self.set(id, Value::Integer(value)) }
    }
    unsafe fn getInt(&self, id: AttrID, value: *mut i64) -> tresult {
        if value.is_null() {
            return kInvalidArgument;
        }
        let Some(key) = (unsafe { key(id) }) else {
            return kInvalidArgument;
        };
        if let Some(Value::Integer(v)) = self.values.borrow().get(&key) {
            unsafe {
                *value = *v;
            }
            kResultOk
        } else {
            kResultFalse
        }
    }
    unsafe fn setFloat(&self, id: AttrID, value: f64) -> tresult {
        unsafe { self.set(id, Value::Float(value)) }
    }
    unsafe fn getFloat(&self, id: AttrID, value: *mut f64) -> tresult {
        if value.is_null() {
            return kInvalidArgument;
        }
        let Some(key) = (unsafe { key(id) }) else {
            return kInvalidArgument;
        };
        if let Some(Value::Float(v)) = self.values.borrow().get(&key) {
            unsafe {
                *value = *v;
            }
            kResultOk
        } else {
            kResultFalse
        }
    }
    unsafe fn setString(&self, id: AttrID, string: *const u16) -> tresult {
        if string.is_null() {
            return kInvalidArgument;
        }
        let mut value = Vec::new();
        for index in 0..4096 {
            let ch = unsafe { *string.add(index) };
            if ch == 0 {
                return unsafe { self.set(id, Value::String(value)) };
            }
            value.push(ch);
        }
        kResultFalse
    }
    unsafe fn getString(&self, id: AttrID, string: *mut u16, size: u32) -> tresult {
        if string.is_null() || size < 2 {
            return kInvalidArgument;
        }
        let Some(key) = (unsafe { key(id) }) else {
            return kInvalidArgument;
        };
        if let Some(Value::String(v)) = self.values.borrow().get(&key) {
            let n = v.len().min(size as usize / 2 - 1);
            unsafe {
                ptr::copy_nonoverlapping(v.as_ptr(), string, n);
                *string.add(n) = 0;
            }
            kResultOk
        } else {
            kResultFalse
        }
    }
    unsafe fn setBinary(&self, id: AttrID, data: *const c_void, size: u32) -> tresult {
        if size as usize > MAX_STATE_BYTES || (size > 0 && data.is_null()) {
            return kInvalidArgument;
        }
        let bytes = if size == 0 {
            vec![]
        } else {
            unsafe { std::slice::from_raw_parts(data.cast::<u8>(), size as usize).to_vec() }
        };
        unsafe { self.set(id, Value::Binary(bytes)) }
    }
    unsafe fn getBinary(&self, id: AttrID, data: *mut *const c_void, size: *mut u32) -> tresult {
        if data.is_null() || size.is_null() {
            return kInvalidArgument;
        }
        let Some(key) = (unsafe { key(id) }) else {
            return kInvalidArgument;
        };
        if let Some(Value::Binary(v)) = self.values.borrow().get(&key) {
            unsafe {
                *data = v.as_ptr().cast();
                *size = v.len() as u32;
            }
            kResultOk
        } else {
            kResultFalse
        }
    }
}
pub(super) unsafe fn create(cid: *mut TUID, iid: *mut TUID, obj: *mut *mut c_void) -> tresult {
    if cid.is_null() || iid.is_null() || obj.is_null() {
        return kInvalidArgument;
    }
    unsafe {
        *obj = ptr::null_mut();
    }
    // QueryInterface supplies the requested owning reference; drop our temporary.
    unsafe {
        let instance = if *cid == IMessage_iid {
            ComWrapper::new(Message {
                id: RefCell::new(CString::default()),
                attributes: ComWrapper::new(Attributes::new()),
            })
            .to_com_ptr::<FUnknown>()
            .unwrap()
        } else if *cid == IAttributeList_iid {
            ComWrapper::new(Attributes::new())
                .to_com_ptr::<FUnknown>()
                .unwrap()
        } else {
            return kNoInterface;
        };
        {
            let raw = instance.as_ptr();
            ((*(*raw).vtbl).queryInterface)(raw, iid, obj)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn message_attributes_round_trip_and_reject_capacity_overflow() {
        unsafe {
            let mut cid = IMessage_iid;
            let mut iid = IMessage_iid;
            let mut object = ptr::null_mut();
            assert_eq!(create(&mut cid, &mut iid, &mut object), kResultOk);
            let message = ComPtr::<IMessage>::from_raw(object.cast()).unwrap();
            message.setMessageID(c"changed".as_ptr());
            assert_eq!(
                CStr::from_ptr(message.getMessageID()).to_bytes(),
                b"changed"
            );
            let attrs = ::vst3::ComRef::from_raw(message.getAttributes()).unwrap();
            assert_eq!(attrs.setInt(c"id".as_ptr(), 19), kResultOk);
            let mut integer = 0;
            assert_eq!(attrs.getInt(c"id".as_ptr(), &mut integer), kResultOk);
            assert_eq!(integer, 19);
            assert_eq!(attrs.setFloat(c"value".as_ptr(), 0.25), kResultOk);
            let mut float = 0.0;
            assert_eq!(attrs.getFloat(c"value".as_ptr(), &mut float), kResultOk);
            assert_eq!(float, 0.25);
            assert_eq!(
                attrs.setString(c"name".as_ptr(), [65, 66, 0].as_ptr()),
                kResultOk
            );
            let mut string = [0; 3];
            assert_eq!(
                attrs.getString(c"name".as_ptr(), string.as_mut_ptr(), 6),
                kResultOk
            );
            assert_eq!(string, [65, 66, 0]);
            assert_eq!(
                attrs.setBinary(c"state".as_ptr(), [3u8, 5].as_ptr().cast(), 2),
                kResultOk
            );
            let mut data = ptr::null();
            let mut length = 0;
            assert_eq!(
                attrs.getBinary(c"state".as_ptr(), &mut data, &mut length),
                kResultOk
            );
            assert_eq!(
                std::slice::from_raw_parts(data.cast::<u8>(), length as usize),
                [3, 5]
            );
            assert_ne!(
                attrs.setBinary(c"large".as_ptr(), ptr::null(), MAX_STATE_BYTES as u32 + 1),
                kResultOk
            );
            assert_eq!(attrs.getInt(c"absent".as_ptr(), &mut integer), kResultFalse);
        }
    }
}
