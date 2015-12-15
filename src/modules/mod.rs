/// Fetches the source of a built-in Lua module by a given name.
///
/// Returns an `Option<&str>` containing the Lua source of the given module, or `None` if the
/// module is not defined.
///
/// The list of available built-in modules is determined at compile-time and is embedded into the
/// Rote binary itself.
pub fn fetch(name: &str) -> Option<&str> {
    match name {
        // Statically include and match the built-in modules.
        "core"      => Some(include_str!("core.lua")),
        "cargo"     => Some(include_str!("cargo.lua")),
        "cpp"       => Some(include_str!("cpp.lua")),
        "docker"    => Some(include_str!("docker.lua")),
        "fs"        => Some(include_str!("fs.lua")),
        "git"       => Some(include_str!("git.lua")),
        "java"      => Some(include_str!("java.lua")),
        "php"       => Some(include_str!("php.lua")),
        "table"     => Some(include_str!("table.lua")),
        // If you want to add a built-in module, add your module name and file here.
        _ => None
    }
}