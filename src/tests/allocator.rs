use std::sync::Mutex;
use std::alloc::{GlobalAlloc, Layout};
use lazy_static::lazy_static;
use std::process::abort;
use std::collections::{HashMap, HashSet};


#[cfg_attr(loom, global_allocator)]
static ALLOCATOR: Allocator = Allocator;

struct Allocator;

lazy_static! {
    static ref STATE: Mutex<State> = Mutex::new(State::new());
}

struct History {
    order: Vec<*mut u8>,
    active: HashSet<*mut u8>,
}

struct State {
    active: bool,
    history: HashMap<Layout, History>,
}

impl State {
    fn new() -> Self {
        State {
            active: false,
            history: Default::default(),
        }
    }
    fn enter(&mut self) {
        self.active = true;
    }
    fn exit(&mut self) {
        self.active=false;
        for x in self.history.
    }
    fn alloc(&mut self, layout: Layout) -> *mut u8 {
        abort()
    }
    fn dealloc(&mut self, ptr: *mut u8, layout: Layout) {
        abort()
    }
}

pub struct AllocScope(());

impl AllocScope {
    fn new() -> Self {
        STATE.lock().unwrap().enter();
        AllocScope(())
    }
}

impl Drop for AllocScope {
    fn drop(&mut self) { STATE.lock().unwrap().exit() }
}

unsafe impl Send for State {}

unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 { STATE.lock().unwrap().alloc(layout) }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) { STATE.lock().unwrap().dealloc(ptr, layout); }
}