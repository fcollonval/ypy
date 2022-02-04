use lib0::any::Any;
use pyo3::prelude::*;
use pyo3::types as pytypes;
use pyo3::types::PyByteArray;
use pyo3::types::PyDict;
use pyo3::types::PyList;
use pyo3::AsPyPointer;
use std::collections::HashMap;
use std::convert::TryFrom;
use std::ops::Deref;
use yrs;
use yrs::block::{ItemContent, Prelim};
use yrs::types::Attrs;
use yrs::types::Change;
use yrs::types::Delta;
use yrs::types::EntryChange;
use yrs::types::Path;
use yrs::types::PathSegment;
use yrs::types::{Branch, BranchRef, TypePtr, Value};
use yrs::{Array, Map, Text, Transaction};

use crate::shared_types::{Shared, SharedType};
use crate::y_array::YArray;
use crate::y_map::YMap;
use crate::y_text::YText;
use crate::y_xml::{YXmlElement, YXmlText};

pub trait ToPython {
    fn into_py(self, py: Python) -> PyObject;
}

impl<T> ToPython for Vec<T>
where
    T: ToPython,
{
    fn into_py(self, py: Python) -> PyObject {
        let elements = self.into_iter().map(|v| v.into_py(py));
        let arr: PyObject = pyo3::types::PyList::new(py, elements).into();
        return arr;
    }
}

impl<K, V> ToPython for HashMap<K, V>
where
    K: ToPyObject,
    V: ToPython,
{
    fn into_py(self, py: Python) -> PyObject {
        let pyDict = PyDict::new(py);
        for (k, v) in self.into_iter() {
            pyDict.set_item(k, v.into_py(py)).unwrap();
        }
        pyDict.into_py(py)
    }
}

/// Converts a Y.rs Path object into a Python object.
pub fn path_into_py(path: Path) -> PyObject {
    Python::with_gil(|py| {
        let result = PyList::empty(py);
        for segment in path {
            match segment {
                PathSegment::Key(key) => {
                    result.append(key.as_ref()).unwrap();
                }
                PathSegment::Index(idx) => {
                    result.append(idx).unwrap();
                }
            }
        }
        result.into()
    })
}

impl ToPython for Delta {
    fn into_py(self, py: Python) -> PyObject {
        let result = PyDict::new(py);
        match self {
            Delta::Inserted(value, attrs) => {
                let value = value.clone().into_py(py);
                result.set_item("insert", value).unwrap();

                if let Some(attrs) = attrs {
                    let attrs = attrs_into_py(attrs.deref());
                    result.set_item("attributes", attrs).unwrap();
                }
            }
            Delta::Retain(len, attrs) => {
                result.set_item("retain", len).unwrap();

                if let Some(attrs) = attrs {
                    let attrs = attrs_into_py(attrs.deref());
                    result.set_item("attributes", attrs).unwrap();
                }
            }
            Delta::Deleted(len) => {
                result.set_item("delete", len).unwrap();
            }
        }
        result.into()
    }
}

fn attrs_into_py(attrs: &Attrs) -> PyObject {
    Python::with_gil(|py| {
        let o = PyDict::new(py);
        for (key, value) in attrs.iter() {
            let key = key.as_ref();
            let value = Value::Any(value.clone()).into_py(py);
            o.set_item(key, value).unwrap();
        }
        o.into()
    })
}

impl ToPython for &Change {
    fn into_py(self, py: Python) -> PyObject {
        let result = PyDict::new(py);
        match self {
            Change::Added(values) => {
                let values: Vec<PyObject> =
                    values.into_iter().map(|v| v.clone().into_py(py)).collect();
                result.set_item("insert", values).unwrap();
            }
            Change::Removed(len) => {
                result.set_item("delete", len).unwrap();
            }
            Change::Retain(len) => {
                result.set_item("retain", len).unwrap();
            }
        }
        result.into()
    }
}

struct EntryChangeWrapper<'a>(&'a EntryChange);

impl<'a> IntoPy<PyObject> for EntryChangeWrapper<'a> {
    fn into_py(self, py: Python) -> PyObject {
        let result = PyDict::new(py);
        let action = "action";
        match self.0 {
            EntryChange::Inserted(new) => {
                let new_value = new.clone().into_py(py);
                result.set_item(action, "add").unwrap();
                result.set_item("newValue", new_value).unwrap();
            }
            EntryChange::Updated(old, new) => {
                let old_value = old.clone().into_py(py);
                let new_value = new.clone().into_py(py);
                result.set_item(action, "update").unwrap();
                result.set_item("oldValue", old_value).unwrap();
                result.set_item("newValue", new_value).unwrap();
            }
            EntryChange::Removed(old) => {
                let old_value = old.clone().into_py(py);
                result.set_item(action, "delete").unwrap();
                result.set_item("oldValue", old_value).unwrap();
            }
        }
        result.into()
    }
}

struct PyObjectWrapper(PyObject);

impl Prelim for PyObjectWrapper {
    fn into_content(self, _txn: &mut Transaction, ptr: TypePtr) -> (ItemContent, Option<Self>) {
        let guard = Python::acquire_gil();
        let py = guard.python();
        let content = if let Some(any) = py_into_any(self.0.clone()) {
            ItemContent::Any(vec![any])
        } else if let Ok(shared) = Shared::extract(self.0.as_ref(py)) {
            if shared.is_prelim() {
                let branch = BranchRef::new(Branch::new(ptr, shared.type_ref(), None));
                ItemContent::Type(branch)
            } else {
                panic!("Cannot integrate this type")
            }
        } else {
            panic!("Cannot integrate this type")
        };

        let this = if let ItemContent::Type(_) = &content {
            Some(self)
        } else {
            None
        };

        (content, this)
    }

    fn integrate(self, txn: &mut Transaction, inner_ref: BranchRef) {
        let guard = Python::acquire_gil();
        let py = guard.python();
        let obj_ref = self.0.as_ref(py);
        if let Ok(shared) = Shared::extract(obj_ref) {
            if shared.is_prelim() {
                Python::with_gil(|py| match shared {
                    Shared::Text(v) => {
                        let text = Text::from(inner_ref);
                        let mut y_text = v.borrow_mut(py);

                        if let SharedType::Prelim(v) = y_text.0.to_owned() {
                            text.push(txn, v.as_str());
                        }
                        y_text.0 = SharedType::Integrated(text.clone());
                    }
                    Shared::Array(v) => {
                        let array = Array::from(inner_ref);
                        let mut y_array = v.borrow_mut(py);
                        if let SharedType::Prelim(items) = y_array.0.to_owned() {
                            let len = array.len();
                            insert_at(&array, txn, len, items);
                        }
                        y_array.0 = SharedType::Integrated(array.clone());
                    }
                    Shared::Map(v) => {
                        let map = Map::from(inner_ref);
                        let mut y_map = v.borrow_mut(py);
                        if let SharedType::Prelim(entries) = y_map.0.to_owned() {
                            for (k, v) in entries {
                                map.insert(txn, k, PyValueWrapper(v));
                            }
                        }
                        y_map.0 = SharedType::Integrated(map.clone());
                    }
                    _ => panic!("Cannot integrate this type"),
                })
            }
        }
    }
}

pub fn insert_at(dst: &Array, txn: &mut Transaction, index: u32, src: Vec<PyObject>) {
    let mut j = index;
    let mut i = 0;
    while i < src.len() {
        let mut anys = Vec::default();
        while i < src.len() {
            if let Some(any) = py_into_any(src[i].clone()) {
                anys.push(any);
                i += 1;
            } else {
                break;
            }
        }

        if !anys.is_empty() {
            let len = anys.len() as u32;
            dst.insert_range(txn, j, anys);
            j += len;
        } else {
            let wrapper = PyObjectWrapper(src[i].clone());
            dst.insert(txn, j, wrapper);
            i += 1;
            j += 1;
        }
    }
}

fn py_into_any(v: PyObject) -> Option<Any> {
    Python::with_gil(|py| -> Option<Any> {
        let v = v.as_ref(py);

        if let Ok(s) = v.downcast::<pytypes::PyString>() {
            let string: String = s.extract().unwrap();
            Some(Any::String(string.into_boxed_str()))
        } else if let Ok(l) = v.downcast::<pytypes::PyLong>() {
            let i: f64 = l.extract().unwrap();
            Some(Any::BigInt(i as i64))
        } else if v == py.None().as_ref(py) {
            Some(Any::Null)
        } else if let Ok(f) = v.downcast::<pytypes::PyFloat>() {
            Some(Any::Number(f.extract().unwrap()))
        } else if let Ok(b) = v.downcast::<pytypes::PyBool>() {
            Some(Any::Bool(b.extract().unwrap()))
        } else if let Ok(list) = v.downcast::<pytypes::PyList>() {
            let mut result = Vec::with_capacity(list.len());
            for value in list.iter() {
                result.push(py_into_any(value.into())?);
            }
            Some(Any::Array(result.into_boxed_slice()))
        } else if let Ok(dict) = v.downcast::<pytypes::PyDict>() {
            if let Ok(_) = Shared::extract(v) {
                None
            } else {
                let mut result = HashMap::new();
                for (k, v) in dict.iter() {
                    let key = k
                        .downcast::<pytypes::PyString>()
                        .unwrap()
                        .extract()
                        .unwrap();
                    let value = py_into_any(v.into())?;
                    result.insert(key, value);
                }
                Some(Any::Map(Box::new(result)))
            }
        } else {
            None
        }
    })
}

impl ToPython for Any {
    fn into_py(self, py: Python) -> pyo3::PyObject {
        match self {
            Any::Null | Any::Undefined => py.None(),
            Any::Bool(v) => v.into_py(py),
            Any::Number(v) => v.into_py(py),
            Any::BigInt(v) => v.into_py(py),
            Any::String(v) => v.into_py(py),
            Any::Buffer(v) => {
                let byte_array = PyByteArray::new(py, v.as_ref());
                byte_array.into()
            }
            Any::Array(v) => {
                let mut a = Vec::new();
                for value in v.iter() {
                    let value = value.to_owned();
                    a.push(value);
                }
                a.into_py(py)
            }
            Any::Map(v) => {
                let mut m = HashMap::new();
                for (k, v) in v.iter() {
                    let value = v.to_owned();
                    m.insert(k, value);
                }
                m.into_py(py)
            }
        }
    }
}

impl ToPython for Value {
    fn into_py(self, py: Python) -> pyo3::PyObject {
        match self {
            Value::Any(v) => v.into_py(py),
            Value::YText(v) => YText::from(v).into_py(py),
            Value::YArray(v) => YArray::from(v).into_py(py),
            Value::YMap(v) => YMap::from(v).into_py(py),
            Value::YXmlElement(v) => YXmlElement(v).into_py(py),
            Value::YXmlText(v) => YXmlText(v).into_py(py),
        }
    }
}

pub struct PyValueWrapper(pub PyObject);

impl Prelim for PyValueWrapper {
    fn into_content(self, _txn: &mut Transaction, ptr: TypePtr) -> (ItemContent, Option<Self>) {
        let content = if let Some(any) = py_into_any(self.0.clone()) {
            ItemContent::Any(vec![any])
        } else if let Ok(shared) = Shared::try_from(self.0.clone()) {
            if shared.is_prelim() {
                let branch = BranchRef::new(Branch::new(ptr, shared.type_ref(), None));
                ItemContent::Type(branch)
            } else {
                panic!("Cannot integrate this type")
            }
        } else {
            panic!("Cannot integrate this type")
        };

        let this = if let ItemContent::Type(_) = &content {
            Some(self)
        } else {
            None
        };

        (content, this)
    }

    fn integrate(self, txn: &mut Transaction, inner_ref: BranchRef) {
        if let Ok(shared) = Shared::try_from(self.0) {
            if shared.is_prelim() {
                Python::with_gil(|py| match shared {
                    Shared::Text(v) => {
                        let text = Text::from(inner_ref);
                        let mut y_text = v.borrow_mut(py);

                        if let SharedType::Prelim(v) = y_text.0.to_owned() {
                            text.push(txn, v.as_str());
                        }
                        y_text.0 = SharedType::Integrated(text.clone());
                    }
                    Shared::Array(v) => {
                        let array = Array::from(inner_ref);
                        let mut y_array = v.borrow_mut(py);
                        if let SharedType::Prelim(items) = y_array.0.to_owned() {
                            let len = array.len();
                            insert_at(&array, txn, len, items);
                        }
                        y_array.0 = SharedType::Integrated(array.clone());
                    }
                    Shared::Map(v) => {
                        let map = Map::from(inner_ref);
                        let mut y_map = v.borrow_mut(py);

                        if let SharedType::Prelim(entries) = y_map.0.to_owned() {
                            for (k, v) in entries {
                                map.insert(txn, k, PyValueWrapper(v));
                            }
                        }
                        y_map.0 = SharedType::Integrated(map.clone());
                    }
                    _ => panic!("Cannot integrate this type"),
                })
            }
        }
    }
}