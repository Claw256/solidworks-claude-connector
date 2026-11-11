//! Native late-bound COM backend (`SldWorks.Application` via IDispatch). Windows only.
//! Only by-value-argument calls are used (no ByRef out-params), which keeps late binding simple:
//! `LoadFile2`, `GetOpenDocumentByName`, `CreateMassProperty`, `GetPartBox`, `SaveAs3`, ...
//! UNVERIFIED against a live SolidWorks in CI; see the ignored integration test.

use crate::*;
use std::ffi::c_void;
use windows::Win32::System::Com::{
    CLSCTX_LOCAL_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CLSIDFromProgID, DISPATCH_FLAGS, DISPPARAMS,
    IDispatch,
};
use windows::Win32::System::Ole::{SafeArrayGetElement, SafeArrayGetLBound, SafeArrayGetUBound};
use windows::Win32::System::Variant::{VARIANT, VT_ARRAY, VT_BSTR, VT_R8};
use windows::core::{BSTR, GUID, HSTRING, PCWSTR};

const LOCALE_USER_DEFAULT: u32 = 0x0400;

fn cerr(method: &str, e: impl std::fmt::Display) -> SwError {
    SwError::Com { method: method.into(), reason: e.to_string() }
}

/// Thin wrapper over an `IDispatch` with by-name invocation.
#[derive(Clone)]
pub struct Disp(pub IDispatch);

impl Disp {
    /// Invoke `name` as method-or-property-get with positional `args` (given in natural order).
    pub fn invoke(&self, name: &str, args: &[VARIANT]) -> SwResult<VARIANT> {
        let wname = HSTRING::from(name);
        let names = [PCWSTR(wname.as_ptr())];
        let mut id = 0i32;
        unsafe { self.0.GetIDsOfNames(&GUID::zeroed(), names.as_ptr(), 1, LOCALE_USER_DEFAULT, &mut id) }
            .map_err(|e| cerr(name, e))?;
        // IDispatch wants arguments in reverse order.
        let mut rev: Vec<VARIANT> = args.iter().rev().cloned().collect();
        let params = DISPPARAMS {
            rgvarg: if rev.is_empty() { std::ptr::null_mut() } else { rev.as_mut_ptr() },
            rgdispidNamedArgs: std::ptr::null_mut(),
            cArgs: rev.len() as u32,
            cNamedArgs: 0,
        };
        let mut out = VARIANT::default();
        unsafe {
            self.0.Invoke(
                id,
                &GUID::zeroed(),
                LOCALE_USER_DEFAULT,
                DISPATCH_FLAGS(3), // DISPATCH_METHOD | DISPATCH_PROPERTYGET
                &params,
                Some(&mut out as *mut VARIANT),
                None,
                None,
            )
        }
        .map_err(|e| cerr(name, e))?;
        Ok(out)
    }
    pub fn obj(&self, name: &str, args: &[VARIANT]) -> SwResult<Option<Disp>> {
        Ok(IDispatch::try_from(&self.invoke(name, args)?).ok().map(Disp))
    }
    pub fn string(&self, name: &str, args: &[VARIANT]) -> SwResult<Option<String>> {
        Ok(BSTR::try_from(&self.invoke(name, args)?).ok().map(|b| b.to_string()))
    }
    pub fn f64(&self, name: &str, args: &[VARIANT]) -> SwResult<Option<f64>> {
        Ok(f64::try_from(&self.invoke(name, args)?).ok())
    }
    pub fn i32(&self, name: &str, args: &[VARIANT]) -> SwResult<Option<i32>> {
        Ok(i32::try_from(&self.invoke(name, args)?).ok())
    }
    pub fn f64_array(&self, name: &str, args: &[VARIANT]) -> SwResult<Vec<f64>> {
        Ok(variant_f64_array(&self.invoke(name, args)?))
    }
    pub fn string_array(&self, name: &str, args: &[VARIANT]) -> SwResult<Vec<String>> {
        Ok(variant_string_array(&self.invoke(name, args)?))
    }
}

fn parray(v: &VARIANT, elem_vt: u16) -> Option<*mut windows::Win32::System::Com::SAFEARRAY> {
    // SAFETY: reading the tagged union according to its discriminant.
    unsafe {
        let a = &v.Anonymous.Anonymous;
        if a.vt.0 != (VT_ARRAY.0 | elem_vt) {
            return None;
        }
        let p = a.Anonymous.parray;
        (!p.is_null()).then_some(p)
    }
}

fn variant_f64_array(v: &VARIANT) -> Vec<f64> {
    let Some(psa) = parray(v, VT_R8.0) else { return vec![] };
    let mut out = vec![];
    unsafe {
        let (Ok(lb), Ok(ub)) = (SafeArrayGetLBound(psa, 1), SafeArrayGetUBound(psa, 1)) else { return out };
        for i in lb..=ub {
            let mut x = 0f64;
            if SafeArrayGetElement(psa, &i, &mut x as *mut f64 as *mut c_void).is_ok() {
                out.push(x);
            }
        }
    }
    out
}

fn variant_string_array(v: &VARIANT) -> Vec<String> {
    let Some(psa) = parray(v, VT_BSTR.0) else { return vec![] };
    let mut out = vec![];
    unsafe {
        let (Ok(lb), Ok(ub)) = (SafeArrayGetLBound(psa, 1), SafeArrayGetUBound(psa, 1)) else { return out };
        for i in lb..=ub {
            let mut b = BSTR::new();
            if SafeArrayGetElement(psa, &i, &mut b as *mut BSTR as *mut c_void).is_ok() {
                out.push(b.to_string());
            }
        }
    }
    out
}

fn vstr(s: &str) -> VARIANT {
    VARIANT::from(s)
}

#[derive(Default)]
pub struct ComBackend {
    sw: Option<Disp>,
}

impl ComBackend {
    pub fn new() -> Self {
        Self::default()
    }
    fn sw(&self) -> SwResult<&Disp> {
        self.sw.as_ref().ok_or(SwError::NotConnected)
    }
    fn doc(&self, path: &Path) -> SwResult<Disp> {
        let p = path.display().to_string();
        self.sw()?
            .obj("GetOpenDocumentByName", &[vstr(&p)])?
            .ok_or(SwError::NotOpen(p))
    }
}

impl SolidWorksBackend for ComBackend {
    fn connect(&mut self) -> SwResult<()> {
        unsafe {
            // S_FALSE / RPC_E_CHANGED_MODE just mean COM is already initialised on this thread.
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let clsid = CLSIDFromProgID(&HSTRING::from("SldWorks.Application")).map_err(|e| SwError::Connect(e.to_string()))?;
            let app: IDispatch = CoCreateInstance(&clsid, None, CLSCTX_LOCAL_SERVER).map_err(|e| SwError::Connect(e.to_string()))?;
            let app = Disp(app);
            let _ = app.invoke("Visible", &[]); // property get; also proves the server answers
            self.sw = Some(app);
        }
        Ok(())
    }

    fn open_document(&mut self, path: &Path) -> SwResult<()> {
        let p = path.display().to_string();
        let doc = self.sw()?.obj("LoadFile2", &[vstr(&p), vstr("r")])?;
        doc.map(|_| ()).ok_or(SwError::Open { path: p, reason: "LoadFile2 returned no document".into() })
    }

    fn close_document(&mut self, path: &Path) -> SwResult<()> {
        let doc = self.doc(path)?;
        let title = doc.string("GetTitle", &[])?.unwrap_or_default();
        self.sw()?.invoke("CloseDoc", &[vstr(&title)])?;
        Ok(())
    }

    fn analyze_model(&mut self, path: &Path) -> SwResult<ModelAnalysis> {
        let doc = self.doc(path)?;
        let document_type = match doc.i32("GetType", &[])?.unwrap_or(1) {
            2 => DocumentType::Assembly,
            3 => DocumentType::Drawing,
            _ => DocumentType::Part,
        };

        let mut features = vec![];
        let mut dimensions = vec![];
        let mut seen = std::collections::HashSet::new();
        let mut feat = doc.obj("FirstFeature", &[])?;
        let mut guard = 0;
        while let Some(f) = feat {
            guard += 1;
            if guard > 5000 {
                break;
            }
            let name = f.string("Name", &[])?.unwrap_or_default();
            let ty = f.string("GetTypeName2", &[])?.unwrap_or_default();
            if !(ty.ends_with("Folder") || ty == "OriginProfileFeature") {
                features.push(FeatureInfo { name: name.clone(), feature_type: ty });
            }
            // Driving dimensions (same traversal SolidPilot uses).
            let mut dd = f.obj("GetFirstDisplayDimension", &[]).ok().flatten();
            let mut g2 = 0;
            while let Some(d) = dd {
                g2 += 1;
                if g2 > 500 {
                    break;
                }
                if let Ok(Some(dim)) = d.obj("GetDimension2", &[VARIANT::from(0i32)]) {
                    let full = dim.string("FullName", &[]).ok().flatten().unwrap_or_default();
                    if seen.insert(full.clone()) {
                        if let Ok(Some(v)) = dim.f64("SystemValue", &[]) {
                            // IDimension.GetType() -> swDimensionType_e. Per the SolidWorks API docs 4 = swAngularDimension
                            // (the enum is NOT in the SolidPilot source, which inlines other swconst values but never
                            // reads this one; it also notes GetType() is unreliable for drawing dims, so for anything
                            // other than 4 we fall back to "linear"). Angular SystemValue is radians.
                            let kind = match dim.i32("GetType", &[]).ok().flatten() {
                                Some(4) => "angle",
                                _ => "linear",
                            };
                            dimensions.push(DimensionInfo { name: full, feature: name.clone(), value_si: v, kind: kind.into() });
                        }
                    }
                }
                dd = f.obj("GetNextDisplayDimension", &[VARIANT::from(d.0.clone())]).ok().flatten();
            }
            feat = f.obj("GetNextFeature", &[])?;
        }

        let mut mass = None;
        let mut bounding_box = None;
        let mut material = None;
        if document_type != DocumentType::Drawing {
            if let Some(ext) = doc.obj("Extension", &[])? {
                if let Ok(Some(mp)) = ext.obj("CreateMassProperty", &[]) {
                    let cg = mp.f64_array("CenterOfMass", &[]).unwrap_or_default();
                    if let (Some(vol), true) = (mp.f64("Volume", &[])?, cg.len() >= 3) {
                        mass = Some(MassProps {
                            volume_m3: vol,
                            surface_area_m2: mp.f64("SurfaceArea", &[])?.unwrap_or(0.0),
                            mass_kg: mp.f64("Mass", &[])?.unwrap_or(0.0),
                            cg: [cg[0], cg[1], cg[2]],
                        });
                    }
                }
            }
        }
        if document_type == DocumentType::Part {
            let b = doc.f64_array("GetPartBox", &[VARIANT::from(true)]).unwrap_or_default();
            if b.len() >= 6 {
                bounding_box = Some(BoundingBox { min: [b[0], b[1], b[2]], max: [b[3], b[4], b[5]] });
            }
            // "DB|Name|Database" -> Name
            material = doc
                .string("MaterialIdName", &[])
                .ok()
                .flatten()
                .and_then(|s| s.split('|').nth(1).map(str::to_string))
                .filter(|s| !s.is_empty());
        }

        let mut custom_properties = vec![];
        if let Ok(Some(ext)) = doc.obj("Extension", &[]) {
            if let Ok(Some(cpm)) = ext.obj("CustomPropertyManager", &[vstr("")]) {
                for n in cpm.string_array("GetNames", &[]).unwrap_or_default() {
                    if let Ok(Some(v)) = cpm.string("Get", &[vstr(&n)]) {
                        custom_properties.push((n, v));
                    }
                }
            }
        }

        Ok(ModelAnalysis { path: path.display().to_string(), document_type, features, dimensions, mass, custom_properties, bounding_box, material })
    }

    fn export(&mut self, path: &Path, format: ExportFormat, out: &Path) -> SwResult<()> {
        let doc = self.doc(path)?;
        let mut out = out.to_path_buf();
        if out.extension().is_none() {
            out.set_extension(format.extension());
        }
        // SaveAs3(Name, Version=0 current, Options=2 silent) -> error code (0 = ok); format by extension.
        let rc = doc
            .i32("SaveAs3", &[vstr(&out.display().to_string()), VARIANT::from(0i32), VARIANT::from(2i32)])
            .map_err(|e| SwError::Export { format: format.extension().into(), reason: e.to_string() })?;
        match rc {
            Some(0) | None if out.exists() => Ok(()),
            Some(c) if c != 0 => Err(SwError::Export { format: format.extension().into(), reason: format!("SaveAs3 error code {c}") }),
            _ => Err(SwError::Export { format: format.extension().into(), reason: "output file not created".into() }),
        }
    }
}
