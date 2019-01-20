use winit::{Event, EventsLoop, KeyboardInput, VirtualKeyCode, WindowBuilder, WindowEvent};
use winit::dpi::LogicalSize;

use image::GenericImageView;

mod hal_state;
mod local_state;
mod user_input;
mod winit_state;

use hal_state::HalState;
use local_state::LocalState;
use user_input::UserInput;
use winit_state::WinitState;

use log::Level;
use log::{debug, error, info, trace, warn};

use std::error::Error;

fn main() -> Result<(), Box<dyn Error>> {
    simple_logger::init_with_level(Level::Warn).unwrap();
    let mut winit_state = winit_state::WinitState::new("NiceGFX window", LogicalSize{ width: 800f64, height: 600f64}.into())?;
    let mut hal_state = hal_state::HalState::new(&winit_state.window)?;

    let (frame_width, frame_height) = winit_state
        .window
        .get_inner_size()
        .map(|logical| logical.into())
        .unwrap_or((0.0, 0.0));

    let mut local_state = local_state::LocalState {
        frame_width,
        frame_height,
        mouse_x: 0.0,
        mouse_y: 0.0,
    };

    loop {
        let input = user_input::UserInput::poll_events_loop(&mut winit_state.events_loop);
        if input.end_requested {
            break;
        }
        if input.new_frame_size.is_some() {
            hal_state = HalState::new(&winit_state.window)?;
        }
        local_state.update_from_input(input);

        if let Err(e) = do_render(&mut hal_state, &mut local_state) {
            error!("{:#?}", e);
        }


    }

    Ok(())
}

fn do_render(hal_state: &mut HalState, local_state: &LocalState) -> Result<(), &'static str> {
    let r = (local_state.mouse_x / local_state.frame_width) as f32;
    let g = (local_state.mouse_y / local_state.frame_height) as f32;
    let b = (r + g) * 0.3;
    let a = 1.0;
    hal_state.draw_clear_frame([r, g, b, a])
}
