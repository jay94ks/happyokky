use std::{env, fs, io, path::PathBuf};

const ICON_FILE: &str = "assets/happy-okky.ico";
const ICON_GROUP_ID: u16 = 1;
const RT_ICON: u16 = 3;
const RT_GROUP_ICON: u16 = 14;

fn main() -> io::Result<()> {
    println!("cargo:rerun-if-changed={ICON_FILE}");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows")
        || env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("msvc")
    {
        return Ok(());
    }
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo output directory"));
    let resource = out.join("happy-okky.res");
    fs::write(&resource, icon_resources(&fs::read(ICON_FILE)?)?)?;
    println!("cargo:rustc-link-arg-bin=happy-okky={}", resource.display());
    Ok(())
}

fn icon_resources(ico: &[u8]) -> io::Result<Vec<u8>> {
    let invalid = || io::Error::new(io::ErrorKind::InvalidData, "invalid Happy Okky icon file");
    let header = ico.get(..6).ok_or_else(invalid)?;
    let count = u16::from_le_bytes([header[4], header[5]]);
    if header[..4] != [0, 0, 1, 0] || count == 0 {
        return Err(invalid());
    }
    let entries = ico
        .get(6..6 + usize::from(count) * 16)
        .ok_or_else(invalid)?;
    let mut resource = Vec::new();
    let mut group = header.to_vec();
    // MSVC resource files begin with an empty header.
    append_resource(&mut resource, 0, 0, &[]);
    for (index, entry) in entries.chunks_exact(16).enumerate() {
        let id = index as u16 + 1;
        let length = u32::from_le_bytes(entry[8..12].try_into().unwrap()) as usize;
        let offset = u32::from_le_bytes(entry[12..16].try_into().unwrap()) as usize;
        let image = offset
            .checked_add(length)
            .and_then(|end| ico.get(offset..end))
            .ok_or_else(invalid)?;
        append_resource(&mut resource, RT_ICON, id, image);
        // A group entry refers to the resource ID in place of the ICO data offset.
        group.extend_from_slice(&entry[..12]);
        group.extend_from_slice(&id.to_le_bytes());
    }
    append_resource(&mut resource, RT_GROUP_ICON, ICON_GROUP_ID, &group);
    Ok(resource)
}

fn append_resource(resource: &mut Vec<u8>, kind: u16, id: u16, data: &[u8]) {
    resource.extend_from_slice(&(data.len() as u32).to_le_bytes());
    resource.extend_from_slice(&32_u32.to_le_bytes());
    for word in [u16::MAX, kind, u16::MAX, id] {
        resource.extend_from_slice(&word.to_le_bytes());
    }
    // Default data version, flags, language, version, and characteristics.
    resource.extend_from_slice(&[0; 16]);
    resource.extend_from_slice(data);
    resource.resize(resource.len().next_multiple_of(4), 0);
}
