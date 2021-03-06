use lua::{self, ffi};
use lua::libc::{c_int, c_void};
use std::any::{Any, TypeId};
use std::clone::Clone;
use std::error::Error;
use std::hash::{Hash, Hasher, SipHasher};
use std::mem;
use std::path::PathBuf;
use std::ptr;
use std::rc::{Rc, Weak};

mod environment;
mod iter;

pub use self::environment::Environment;

/// Results that are returned by functions callable from Lua.
pub type ScriptResult = Result<i32, Box<Error>>;

/// A function that can be bound to be callable inside the Lua runtime.
pub type Function = fn(Runtime) -> ScriptResult;
pub type Closure = FnMut(Runtime) -> ScriptResult;


/// Manages the Lua interpreter runtime for a Lua script.
pub struct Runtime {
    /// Shared environment between all references to the runtime.
    environment: Rc<Environment>,

    /// A Lua interpreter state.
    state: lua::State,
}

impl Runtime {
    /// Creates a new environment for a given script file.
    ///
    /// The instance is placed inside a box to ensure the runner has a constant location in memory
    /// so that it can be referenced by native closures in the runtime.
    pub fn new(environment: Environment) -> Runtime {
        let runtime = Runtime {
            environment: Rc::new(environment),
            state: lua::State::new(),
        };

        // Create a weak pointer to the environment and push it into the registry so that you can
        // access the environment object by its Lua state.
        runtime.state().push(type_id_of::<Environment>() as f64);
        let ptr = runtime.state().new_userdata_typed();
        let weak = Rc::downgrade(&runtime.environment);
        unsafe {
            ptr::write(ptr, weak);
        }
        runtime.state().set_table(lua::REGISTRYINDEX);

        runtime
    }

    /// Fetches the environment by its Lua pointer.
    ///
    /// This function is far from safe, and should be used with care.
    pub unsafe fn from_ptr(ptr: *mut ffi::lua_State) -> Runtime {
        // First, get a State object from the raw pointer.
        let mut state = lua::State::from_ptr(ptr);

        // Fetch the environment pointer from the registry.
        let environment = {
            state.push(type_id_of::<Environment>() as f64);
            state.get_table(lua::REGISTRYINDEX);

            // Read the weak pointer.
            let weak = match state.to_userdata_typed::<Weak<Environment>>(-1) {
                Some(weak) => {
                    lua::State::from_ptr(ptr).pop(1);
                    weak
                },
                None => panic!("unable to read environment pointer")
            };

            match Weak::upgrade(&weak) {
                Some(rc) => rc,
                None => panic!("unable to upgrade environment pointer")
            }
        };

        // Upgrade the pointer and convert it to an environment.
        Runtime {
            environment: environment,
            state: state,
        }
    }

    /// Gets the runtime environment.
    pub fn environment(&self) -> &Environment {
        &self.environment
    }

    /// Gets a mutable instance of the Lua interpreter state.
    ///
    /// This function uses the direct lua_State pointer, so multiple owners can all mutate the same
    /// state simultaneously. Obviously this is a little unsafe, so use responsibly.
    pub fn state(&self) -> lua::State {
        unsafe {
            lua::State::from_ptr(self.state.as_ptr())
        }
    }

    /// Executes the script.
    pub fn load(&self) -> Result<(), Box<Error>> {
        let path_str = if let Some(s) = self.environment.path().to_str() {
            s
        } else {
            return Err("path contains invalid characters".into());
        };

        // Load the given file.
        match self.state().do_file(path_str) {
            lua::ThreadStatus::Ok => {}
            lua::ThreadStatus::FileError => {
                return Err(format!("the file \"{}\" could not be read", path_str).into());
            }
            _ => {
                return Err(self.state().to_str(-1).unwrap().into());
            }
        };

        Ok(())
    }

    /// Adds a path to Lua's require path for modules.
    pub fn include_path<P: Into<PathBuf>>(&self, path: P) {
        let mut lua_path = path.into();
        let mut native_path = lua_path.clone();
        lua_path.push("?.lua");
        native_path.push("?.so");

        self.state().get_global("package");

        // Set the Lua file path.
        self.state().get_field(-1, "path");
        let mut search_path = self.state().to_str(-1).unwrap().to_string();
        search_path.push(';');
        search_path.push_str(&lua_path.to_string_lossy());

        self.state().push_string(&search_path);
        self.state().set_field(-4, "path");
        self.state().pop(2);

        // Set the native file path.
        self.state().get_field(-1, "cpath");
        let mut search_path = self.state().to_str(-1).unwrap().to_string();
        search_path.push(';');
        search_path.push_str(&native_path.to_string_lossy());

        self.state().push_string(&search_path);
        self.state().set_field(-4, "cpath");
        self.state().pop(3);
    }

    /// Evaluates a Lua string inside the runtime.
    pub fn eval<S: AsRef<str>>(&self, code: S) -> Result<(), Box<Error>> {
        if self.state().do_string(code.as_ref()).is_err() {
            return Err(self.state().to_str(-1).unwrap().into());
        }

        Ok(())
    }

    /// Registers a global function in the runtime that can be called by Lua scripts.
    pub fn register_fn(&self, name: &str, f: Function) {
        self.push_fn(f);
        self.state().set_global(name);
    }

    /// Registers a module using preloading.
    pub fn register_lib<S: AsRef<str>>(&self, name: S, loader: Function) {
        self.state().get_global("package");
        self.state().get_field(-1, "preload");

        self.push_fn(loader);

        self.state().set_field(-2, name.as_ref());
        self.state().pop(2);
    }

    /// Loads a table of functions as a module.
    pub fn load_lib(&self, mtable: &[(&str, Function)]) {
        self.state().create_table(0, mtable.len() as i32);

        for &(name, func) in mtable {
            self.push_fn(func);
            self.state().set_field(-2, name);
        }
    }

    /// Pushes a safe Rust function onto the stack.
    pub fn push_fn(&self, function: Function) {
        unsafe {
            // Push a pointer to the given function so that we know what function to delegate to.
            self.state().push_light_userdata(function as *mut c_void);
        }

        // Push a wrapper function onto the stack, which delegates to the given function.
        self.state().push_closure(Some(fn_wrapper), 1);

        // Wrapper function for invoking Rust functions from inside Lua.
        unsafe extern fn fn_wrapper(ptr: *mut ffi::lua_State) -> c_int {
            // Get the runtime from the raw pointer.
            let runtime = Runtime::from_ptr(ptr);

            // Get the raw pointer and turn it back into a Rust function pointer.
            let fn_ptr = runtime.state().to_userdata(ffi::lua_upvalueindex(1));
            let function: Function = mem::transmute(fn_ptr);

            // Invoke the function.
            function(runtime).unwrap_or_else(|err: Box<Error>| {
                let mut state = lua::State::from_ptr(ptr);

                state.location(1);
                state.push_string(err.description());
                state.concat(2);
                state.error();
            }) as c_int
        }
    }

    /// Pushes a safe Rust closure onto the stack.
    pub fn push_closure(&self, closure: Box<Closure>) {
        unsafe {
            // Push a pointer to the closure so that we know what to delegate to.
            let closure_ptr = self.state().new_userdata_typed();
            ptr::write(closure_ptr, Box::into_raw(closure));

            // Tell Lua how to clean up the closure.
            if self.state().get_metatable(-1) {
                self.state().push_fn(Some(drop_closure));
                self.state().set_field(-2, "__gc");
                self.state().pop(1);
            }
        }

        // Push a wrapper function onto the stack, which delegates to the given function.
        self.state().push_closure(Some(closure_wrapper), 1);

        // Wrapper function for invoking Rust functions from inside Lua.
        unsafe extern fn closure_wrapper(ptr: *mut ffi::lua_State) -> c_int {
            // Get the runtime from the raw pointer.
            let runtime = Runtime::from_ptr(ptr);

            // Get the upvalue and turn it back into a Rust closure pointer.
            let closure_ptr = runtime.state().to_userdata(ffi::lua_upvalueindex(1));
            let closure: *mut *mut Closure = mem::transmute(closure_ptr);

            // Invoke the closure.
            (**closure)(runtime).unwrap_or_else(|err: Box<Error>| {
                let mut state = lua::State::from_ptr(ptr);

                state.location(1);
                state.push_string(err.description());
                state.concat(2);
                state.error();
            }) as c_int
        }

        // Cleans up the memory of a closure.
        unsafe extern fn drop_closure(ptr: *mut ffi::lua_State) -> c_int {
            let mut state = lua::State::from_ptr(ptr);

            // Get the closure to free.
            let closure_ptr = state.to_userdata(1);
            let closure: *mut *mut Closure = mem::transmute(closure_ptr);
            let closure_box = Box::from_raw(*closure);

            // Free the closure.
            drop(closure_box);

            0
        }
    }

    /// Returns an iterator for iterating over the table at the top of the stack.
    pub fn iter(&self, index: lua::Index) -> iter::TableIterator {
        iter::TableIterator::new(self.state(), index)
    }

    /// Wrapper around `lua_pcall()` that catches errors as a result.
    pub fn call(&self, nargs: i32, nresults: i32, msgh: i32) -> Result<lua::ThreadStatus, Box<Error>> {
        let status = self.state().pcall(nargs, nresults, msgh);

        if status.is_err() {
            if self.state().is_string(-1) {
                Err(self.state().to_str(-1).unwrap().to_string().into())
            } else {
                Err("unknown error".into())
            }
        } else {
            Ok(status)
        }
    }

    /// Pushes the value of a registry key onto the stack.
    pub fn reg_get(&self, name: &str) {
        self.state().push(name);
        self.state().get_table(lua::REGISTRYINDEX);
    }

    /// Sets a registry key to the value at the top of the stack.
    pub fn reg_set(&self, name: &str) {
        self.state().push(name);
        self.state().push_value(-2);
        self.state().set_table(lua::REGISTRYINDEX);
        self.state().pop(1);
    }
}

/// Implement cloning for runtime references, with the same semantics as an Rc.
impl Clone for Runtime {
    fn clone(&self) -> Self {
        Runtime {
            environment: self.environment.clone(),
            state: self.state(),
        }
    }
}

/// Safe type ID numeric function.
fn type_id_of<T: Any>() -> u64 {
    let type_id = TypeId::of::<T>();
    let mut hasher = SipHasher::new();
    type_id.hash(&mut hasher);
    hasher.finish()
}
