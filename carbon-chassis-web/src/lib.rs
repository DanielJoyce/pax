//! Basic example of rendering in the browser

use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{window, HtmlCanvasElement};
use std::rc::Rc;
use std::cell::RefCell;

use piet_web::WebRenderContext;

use carbon::CarbonEngine;

fn browser_window() -> web_sys::Window {
    web_sys::window().expect("no global `window` exists")
}

fn request_animation_frame(f: &Closure<dyn FnMut()>) {
    browser_window()
        .request_animation_frame(f.as_ref().unchecked_ref())
        .expect("should register `requestAnimationFrame` OK");
}

// Console.log support
#[wasm_bindgen]
extern "C" {
    // Use `js_namespace` here to bind `console.log(..)` instead of just
    // `log(..)`
    #[wasm_bindgen(js_namespace = console)]
    fn log(s: &str);

    // The `console.log` is quite polymorphic, so we can bind it with multiple
    // signatures. Note that we need to use `js_name` to ensure we always call
    // `log` in JS.
    #[wasm_bindgen(js_namespace = console, js_name = log)]
    fn log_u32(a: u32);

    // Multiple arguments too!
    #[wasm_bindgen(js_namespace = console, js_name = log)]
    fn log_many(a: &str, b: &str);
}

macro_rules! console_log {
    ($($t:tt)*) => (log(&format_args!($($t)*).to_string()))
}

pub fn log_wrapper(msg: &str) {
    console_log!("{}", msg);
}

#[wasm_bindgen]
pub fn run() {
    #[cfg(feature = "console_error_panic_hook")]
    std::panic::set_hook(Box::new(console_error_panic_hook::hook));

    let window = window().unwrap();
    let canvas = window
        .document()
        .unwrap()
        .get_element_by_id("canvas")
        .unwrap()
        .dyn_into::<HtmlCanvasElement>()
        .unwrap();
    let context : web_sys::CanvasRenderingContext2d = canvas
        .get_context("2d")
        .unwrap()
        .unwrap()
        .dyn_into::<web_sys::CanvasRenderingContext2d>()
        .unwrap();

    let dpr = window.device_pixel_ratio();
    let width = canvas.offset_width() as f64 * dpr;
    let height = canvas.offset_height() as f64 * dpr;
    //TODO:  update these values on window resize
    //       future:  update these values on _element_ resize
    canvas.set_width(width as u32);
    canvas.set_height(height as u32);

    let _ = context.scale(dpr, dpr);

    let piet_context  = WebRenderContext::new(context, window);
    // piet_context.
    let engine = carbon::get_engine(log_wrapper, (width / dpr, height / dpr));

    let engine_container : Rc<RefCell<CarbonEngine>> = Rc::new(RefCell::new(engine));

    //see web-sys docs for handling browser events with closures
    //https://rustwasm.github.io/docs/wasm-bindgen/examples/closures.html
    {
        let engine_rc_pointer = engine_container.clone();
        let closure = Closure::wrap(Box::new(move |_event: web_sys::Event| {
            let mut engine = engine_rc_pointer.borrow_mut();

            //TODO:  can probably tackle this more elegantly by reusing / capturing / Rc-ing
            //       previously declared window / canvas / context / etc.
            let inner_window = web_sys::window().unwrap();
            // let inner_canvas = inner_window
            //     .document()
            //     .unwrap()
            //     .get_element_by_id("canvas")
            //     .unwrap()
            //     .dyn_into::<HtmlCanvasElement>()
            //     .unwrap();
            // let inner_context = inner_canvas
            //     .get_context("2d")
            //     .unwrap()
            //     .unwrap()
            //     .dyn_into::<web_sys::CanvasRenderingContext2d>()
            //     .unwrap();

            //inner_width and inner_height already account for device pixel ratio.
            let width = inner_window.inner_width().unwrap().as_f64().unwrap();
            let height = inner_window.inner_height().unwrap().as_f64().unwrap();
            let _ = canvas.set_attribute("width", format!("{}",width).as_str());
            let _ = canvas.set_attribute("height", format!("{}",height).as_str());
            console_log!("width: {}", width);
            engine.set_viewport_size((width, height));
        }) as Box<dyn FnMut(_)>);
        let inner_window = web_sys::window().unwrap();
        let _ = inner_window.add_event_listener_with_callback("resize", closure.as_ref().unchecked_ref());
        closure.forget();
    }

    render_loop(&engine_container, piet_context);
}

pub fn render_loop(engine_container: &Rc<RefCell<CarbonEngine>>, mut piet_context: WebRenderContext<'static>) {
    //this special Rc dance lets us kick off the initial rAF loop (`g`)
    //AND fire the subsequent rAF calls (`f`)
    let f = Rc::new(RefCell::new(None));
    let g = f.clone();
    {
        let engine_rc_pointer = engine_container.clone();

        let closure = Closure::wrap(Box::new(move || {
            let mut engine = engine_rc_pointer.borrow_mut();
            engine.tick(&mut piet_context);
            request_animation_frame(f.borrow().as_ref().unwrap());
            // let _ = f.borrow_mut().take(); // clean up
        }) as Box<dyn FnMut()>);

        *g.borrow_mut() = Some(closure);

    }

    //kick off first rAF
    request_animation_frame(g.borrow().as_ref().unwrap());

}