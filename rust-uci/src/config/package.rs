use std::{
    collections::HashMap,
    ffi::{CStr, CString},
    option::Option as StdOption,
    ptr,
    sync::{Arc, Mutex},
};

use libuci_sys::{uci_commit, uci_element, uci_save, uci_to_section, uci_type_UCI_TYPE_PACKAGE};

use crate::{
    config::{handle_error, section::SectionIdent},
    error::Error,
    libuci_locked, Result, Uci,
};

use super::{
    ptr::{UciListIter, UciPtr},
    section::Section,
};

/// represents a single package in the config tree
/// parent to different [Section]s
pub struct Package {
    uci: Arc<Mutex<Uci>>,
    name: Arc<CString>,
}

impl Package {
    pub(crate) fn new(uci: Arc<Mutex<Uci>>, name: Arc<CString>) -> Package {
        Package { uci, name }
    }

    pub fn name(&self) -> Result<&str> {
        Ok(self.name.to_str()?)
    }

    pub(crate) fn ptr_opt<'a>(&'_ self, uci: &'a mut Uci) -> Result<StdOption<UciPtr<'a>>> {
        let mut ptr = UciPtr::new();
        ptr.target = uci_type_UCI_TYPE_PACKAGE;
        ptr.package = self.name.as_c_str().as_ptr();
        ptr.lookup(uci)
    }

    pub(crate) fn ptr<'a>(&'_ self, uci: &'a mut Uci) -> Result<UciPtr<'a>> {
        match self.ptr_opt(uci)? {
            Some(ptr) => Ok(ptr),
            None => Err(Error::EntryNotFound {
                entry_identifier: self.name()?.to_owned(),
            }),
        }
    }

    fn sections_impl(&self) -> Result<impl Iterator<Item = Section>> {
        let mut uci = self.uci.lock().unwrap();

        let ptr = match self.ptr_opt(&mut uci)? {
            Some(ptr) => unsafe { &(*ptr.p).sections },
            None => ptr::null(),
        };

        drop(uci);

        let uci = Arc::clone(&self.uci);
        let package = Arc::clone(&self.name);

        // Anonymous section indexes are counted independently for each section
        // type, matching UCI's @type[index] selector semantics.
        let mut anonymous_indices = HashMap::<Vec<u8>, i32>::new();

        Ok(UciListIter::new(ptr).map(move |elem| {
            let section = unsafe { uci_to_section(elem) };
            let type_ = unsafe { CStr::from_ptr((*section).type_) }.to_owned();

            let ident = if unsafe { (*section).anonymous } {
                let next_index = anonymous_indices
                    .entry(type_.as_bytes().to_vec())
                    .or_default();

                let ident = SectionIdent::Indexed(*next_index);
                *next_index += 1;

                ident
            } else {
                let name = unsafe { CStr::from_ptr((*elem).name) }.to_owned();
                SectionIdent::Named(name)
            };

            Section::new(
                Arc::clone(&uci),
                Arc::clone(&package),
                Arc::new(type_),
                Arc::new(ident),
            )
        }))
    }

    pub fn sections(&self) -> Result<impl Iterator<Item = Section>> {
        self.sections_impl()
    }

    pub fn sections_by_type(
        &self,
        type_: impl AsRef<str>,
    ) -> Result<impl Iterator<Item = Section>> {
        let type_ = CString::new(type_.as_ref())?;

        Ok(self
            .sections_impl()?
            .filter(move |section| section.type_().as_bytes() == type_.as_bytes()))
    }

    /// return a single [Section] by its name
    /// also works if the section is not defined yet
    pub fn section<T: AsRef<str>>(
        &self,
        type_: impl AsRef<str>,
        ident: impl Into<SectionIdent<T>>,
    ) -> Result<Section> {
        let type_ = CString::new(type_.as_ref())?;

        use SectionIdent::*;
        let ident = match ident.into() {
            Anonymous => Anonymous,
            Indexed(i) => Indexed(i),
            Named(n) => Named(CString::new(n.as_ref())?),
        };

        Ok(Section::new(
            Arc::clone(&self.uci),
            Arc::clone(&self.name),
            Arc::new(type_),
            Arc::new(ident),
        ))
    }

    /// save package delta to disk
    pub fn save(&mut self) -> Result<()> {
        let mut uci = self.uci.lock().unwrap();
        let pkg = self.ptr(&mut uci)?.p;
        let result = libuci_locked!(unsafe { uci_save(uci.ctx, pkg) });
        handle_error(&mut uci, result)?;
        Ok(())
    }

    /// commit package delta into real config on disk
    pub fn commit(&mut self) -> Result<()> {
        let mut uci = self.uci.lock().unwrap();
        let mut pkg = self.ptr(&mut uci)?.p;
        // the uci cli seems to set `override=false` too, not sure what it means
        let result = libuci_locked!(unsafe { uci_commit(uci.ctx, &raw mut pkg, false) });
        handle_error(&mut uci, result)?;
        Ok(())
    }
}
