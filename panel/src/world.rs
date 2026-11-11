//! The Minecraft version a world was last saved with, from `level.dat` (`Data.Version.Name`).

use std::path::PathBuf;

use crate::backup;
use crate::instances::Instance;
use crate::nbt;

pub fn level_dat(inst: &Instance) -> PathBuf {
    inst.server_dir()
        .join(backup::level_name(inst))
        .join("level.dat")
}

/// Minecraft version that last saved the world, or None if unknown / no world.
pub fn saved_version(inst: &Instance) -> Option<String> {
    let (_, root) = nbt::load(&level_dat(inst)).ok()?;
    root.compound("Data")?
        .compound("Version")?
        .string("Name")
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_version_name() {
        let _t = crate::testutil::data_dir();
        let inst = Instance::main();
        assert_eq!(saved_version(&inst), None);
        std::fs::create_dir_all(level_dat(&inst).parent().unwrap()).unwrap();
        nbt::save(&level_dat(&inst), "", &nbt::tests::sample()).unwrap();
        assert_eq!(saved_version(&inst).as_deref(), Some("1.21.4"));
        std::fs::write(level_dat(&inst), b"garbage").unwrap();
        assert_eq!(saved_version(&inst), None);
    }
}
