use winit::CreationError;
use winit::EventsLoop;
use winit::Window;
use winit::WindowBuilder;

pub type WindowSize = (u32, u32);

const WINDOW_NAME: &str = "NiceGfx Window";

#[derive(Debug)]
pub struct WinitState {
    pub events_loop: EventsLoop,
    pub window: Window,
}

impl WinitState {
    pub fn new<T: Into<String>>(title: T, size: WindowSize) -> Result<Self, CreationError> {
        let events_loop = EventsLoop::new();
        let window = WindowBuilder::new()
            .with_title(title)
            .with_dimensions(size.into())
            .with_always_on_top(true)
            .build(&events_loop);
        window.map(|window| Self {
            events_loop,
            window,
        })
    }
}

impl Default for WinitState {
    fn default() -> Self {
        Self::new(WINDOW_NAME, (800, 600)).expect("Failed to create a default window")
    }
}
