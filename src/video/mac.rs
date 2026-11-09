//! macOS: mpv draws through its OpenGL render API into a `CAOpenGLLayer`,
//! hosted by a view placed below GPUI's view. GPUI's Metal layer isn't
//! opaque and is cleared to transparent, so the video shows wherever the UI
//! doesn't draw, as with DirectComposition on Windows.
//!
//! mpv's `--wid` isn't supported on macOS, hence the render API.

use std::ffi::{c_char, c_int, c_void};
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use anyhow::{Result, anyhow, bail};
use gpui::Window;
use libmpv2::{Mpv, MpvInitializer};
use libmpv2_sys as sys;
use objc::declare::ClassDecl;
use objc::runtime::{BOOL, Class, NO, Object, Sel, YES};
use objc::{class, msg_send, sel, sel_impl};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

type Id = *mut Object;
type CGLContextObj = *mut c_void;
type CGLPixelFormatObj = *mut c_void;

#[link(name = "OpenGL", kind = "framework")]
unsafe extern "C" {
    fn CGLChoosePixelFormat(
        attribs: *const c_int,
        pix: *mut CGLPixelFormatObj,
        npix: *mut c_int,
    ) -> c_int;
    fn CGLCreateContext(
        pix: CGLPixelFormatObj,
        share: CGLContextObj,
        ctx: *mut CGLContextObj,
    ) -> c_int;
    fn CGLRetainContext(ctx: CGLContextObj) -> CGLContextObj;
    fn CGLReleaseContext(ctx: CGLContextObj);
    fn CGLRetainPixelFormat(pix: CGLPixelFormatObj) -> CGLPixelFormatObj;
    fn CGLReleasePixelFormat(pix: CGLPixelFormatObj);
    fn CGLGetCurrentContext() -> CGLContextObj;
    fn CGLSetCurrentContext(ctx: CGLContextObj) -> c_int;
    fn CGLLockContext(ctx: CGLContextObj) -> c_int;
    fn CGLUnlockContext(ctx: CGLContextObj) -> c_int;
    fn glGetIntegerv(pname: u32, data: *mut c_int);
}

#[link(name = "QuartzCore", kind = "framework")]
unsafe extern "C" {}

unsafe extern "C" {
    fn dlopen(path: *const c_char, mode: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, name: *const c_char) -> *mut c_void;
}

const GL_VIEWPORT: u32 = 0x0BA2;
const GL_DRAW_FRAMEBUFFER_BINDING: u32 = 0x8CA6;

// CGLPixelFormatAttribute values.
const PFA_DOUBLE_BUFFER: c_int = 5;
const PFA_ACCELERATED: c_int = 73;
const PFA_ALLOW_OFFLINE_RENDERERS: c_int = 96;
const PFA_OPENGL_PROFILE: c_int = 99;
const PFA_AUTOMATIC_GRAPHICS_SWITCHING: c_int = 101;
const OPENGL_PROFILE_3_2_CORE: c_int = 0x3200;

const NS_WINDOW_BELOW: isize = -1;
const NS_VIEW_WIDTH_SIZABLE: usize = 2;
const NS_VIEW_HEIGHT_SIZABLE: usize = 16;

#[repr(C)]
#[derive(Clone, Copy)]
struct NSRect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

/// mpv's render context, behind a lock: Core Animation draws on its own
/// thread, and the context is freed from the main thread.
struct RenderContext(*mut sys::mpv_render_context);

// The pointer is only used under the lock (and mpv's render API may be used
// from any thread, one at a time).
unsafe impl Send for RenderContext {}

struct Renderer {
    cgl: CGLContextObj,
    pixel_format: CGLPixelFormatObj,
    /// Null once freed.
    render: Mutex<RenderContext>,
    /// Set by mpv's update callback: a frame may be waiting.
    update: AtomicBool,
    /// Draw even without a new frame (the layer was resized).
    redraw: AtomicBool,
}

unsafe impl Send for Renderer {}
unsafe impl Sync for Renderer {}

impl Drop for Renderer {
    fn drop(&mut self) {
        unsafe {
            CGLReleaseContext(self.cgl);
            CGLReleasePixelFormat(self.pixel_format);
        }
    }
}

impl Renderer {
    /// True if a draw is due; asks mpv whether its update means a new frame.
    fn wants_draw(&self) -> bool {
        let redraw = self.redraw.swap(false, Ordering::AcqRel);
        let update = self.update.swap(false, Ordering::AcqRel);
        let render = self.render.lock().unwrap();
        if render.0.is_null() {
            return false;
        }
        let frame = update
            && unsafe { sys::mpv_render_context_update(render.0) }
                & sys::mpv_render_update_flag_MPV_RENDER_UPDATE_FRAME as u64
                != 0;
        redraw || frame
    }

    /// Renders into the framebuffer bound by Core Animation (its context is
    /// current and locked while the layer draws).
    fn draw(&self) {
        let render = self.render.lock().unwrap();
        if render.0.is_null() {
            return;
        }
        let mut fbo: c_int = 0;
        let mut viewport: [c_int; 4] = [0; 4];
        unsafe {
            glGetIntegerv(GL_DRAW_FRAMEBUFFER_BINDING, &mut fbo);
            glGetIntegerv(GL_VIEWPORT, viewport.as_mut_ptr());
        }
        let mut target = sys::mpv_opengl_fbo {
            fbo,
            w: viewport[2],
            h: viewport[3],
            internal_format: 0,
        };
        // The default framebuffer is bottom-up.
        let mut flip: c_int = 1;
        let mut params = [
            sys::mpv_render_param {
                type_: sys::mpv_render_param_type_MPV_RENDER_PARAM_OPENGL_FBO,
                data: (&raw mut target).cast(),
            },
            sys::mpv_render_param {
                type_: sys::mpv_render_param_type_MPV_RENDER_PARAM_FLIP_Y,
                data: (&raw mut flip).cast(),
            },
            sys::mpv_render_param {
                type_: 0,
                data: ptr::null_mut(),
            },
        ];
        unsafe {
            sys::mpv_render_context_render(render.0, params.as_mut_ptr());
        }
    }

    /// Frees mpv's render context; it must go before mpv itself does.
    fn free(&self) {
        let mut render = self.render.lock().unwrap();
        if render.0.is_null() {
            return;
        }
        unsafe {
            CGLLockContext(self.cgl);
            let previous = CGLGetCurrentContext();
            CGLSetCurrentContext(self.cgl);
            sys::mpv_render_context_set_update_callback(render.0, None, ptr::null_mut());
            sys::mpv_render_context_free(render.0);
            CGLSetCurrentContext(previous);
            CGLUnlockContext(self.cgl);
        }
        render.0 = ptr::null_mut();
    }
}

unsafe extern "C" fn on_update(context: *mut c_void) {
    let renderer = unsafe { &*(context as *const Renderer) };
    renderer.update.store(true, Ordering::Release);
}

/// GL function lookup for mpv.
unsafe extern "C" fn get_proc_address(_: *mut c_void, name: *const c_char) -> *mut c_void {
    static OPENGL: OnceLock<usize> = OnceLock::new();
    let handle = *OPENGL.get_or_init(|| unsafe {
        // RTLD_LAZY | RTLD_GLOBAL
        dlopen(
            c"/System/Library/Frameworks/OpenGL.framework/OpenGL".as_ptr(),
            0x1 | 0x8,
        ) as usize
    });
    if handle == 0 {
        return ptr::null_mut();
    }
    unsafe { dlsym(handle as *mut c_void, name) }
}

const RENDERER_IVAR: &str = "videoPlayerRenderer";

/// The layer's renderer. The layer holds a reference, released in `dealloc`.
fn layer_renderer(layer: &Object) -> Option<&Renderer> {
    let raw: *const c_void = unsafe { *layer.get_ivar(RENDERER_IVAR) };
    (!raw.is_null()).then(|| unsafe { &*(raw as *const Renderer) })
}

extern "C" fn copy_pixel_format(this: &Object, _: Sel, _mask: u32) -> *mut c_void {
    match layer_renderer(this) {
        Some(r) => unsafe { CGLRetainPixelFormat(r.pixel_format) },
        None => ptr::null_mut(),
    }
}

extern "C" fn copy_context(this: &Object, _: Sel, _pf: *mut c_void) -> *mut c_void {
    match layer_renderer(this) {
        Some(r) => unsafe { CGLRetainContext(r.cgl) },
        None => ptr::null_mut(),
    }
}

extern "C" fn can_draw(
    this: &Object,
    _: Sel,
    _ctx: *mut c_void,
    _pf: *mut c_void,
    _time: f64,
    _timestamp: *const c_void,
) -> BOOL {
    match layer_renderer(this) {
        Some(r) if r.wants_draw() => YES,
        _ => NO,
    }
}

extern "C" fn draw(
    this: &Object,
    _: Sel,
    ctx: *mut c_void,
    pf: *mut c_void,
    time: f64,
    timestamp: *const c_void,
) {
    if let Some(r) = layer_renderer(this) {
        r.draw();
    }
    // The default implementation flushes.
    unsafe {
        let _: () = msg_send![super(this, class!(CAOpenGLLayer)),
            drawInCGLContext: ctx
            pixelFormat: pf
            forLayerTime: time
            displayTime: timestamp];
    }
}

extern "C" fn dealloc(this: &mut Object, _: Sel) {
    unsafe {
        let raw: *const c_void = *this.get_ivar(RENDERER_IVAR);
        if !raw.is_null() {
            drop(Arc::from_raw(raw as *const Renderer));
            this.set_ivar(RENDERER_IVAR, ptr::null::<c_void>());
        }
        let _: () = msg_send![super(this, class!(CAOpenGLLayer)), dealloc];
    }
}

fn layer_class() -> &'static Class {
    static CLASS: OnceLock<usize> = OnceLock::new();
    let class = *CLASS.get_or_init(|| {
        let mut decl = ClassDecl::new("VideoPlayerGLLayer", class!(CAOpenGLLayer))
            .expect("VideoPlayerGLLayer is declared once");
        decl.add_ivar::<*const c_void>(RENDERER_IVAR);
        unsafe {
            decl.add_method(
                sel!(copyCGLPixelFormatForDisplayMask:),
                copy_pixel_format as extern "C" fn(&Object, Sel, u32) -> *mut c_void,
            );
            decl.add_method(
                sel!(copyCGLContextForPixelFormat:),
                copy_context as extern "C" fn(&Object, Sel, *mut c_void) -> *mut c_void,
            );
            decl.add_method(
                sel!(canDrawInCGLContext:pixelFormat:forLayerTime:displayTime:),
                can_draw
                    as extern "C" fn(
                        &Object,
                        Sel,
                        *mut c_void,
                        *mut c_void,
                        f64,
                        *const c_void,
                    ) -> BOOL,
            );
            decl.add_method(
                sel!(drawInCGLContext:pixelFormat:forLayerTime:displayTime:),
                draw as extern "C" fn(&Object, Sel, *mut c_void, *mut c_void, f64, *const c_void),
            );
            decl.add_method(sel!(dealloc), dealloc as extern "C" fn(&mut Object, Sel));
        }
        decl.register() as *const Class as usize
    });
    unsafe { &*(class as *const Class) }
}

fn create_gl_context() -> Result<(CGLContextObj, CGLPixelFormatObj)> {
    let attempts: [&[c_int]; 2] = [
        &[
            PFA_OPENGL_PROFILE,
            OPENGL_PROFILE_3_2_CORE,
            PFA_ACCELERATED,
            PFA_DOUBLE_BUFFER,
            PFA_ALLOW_OFFLINE_RENDERERS,
            PFA_AUTOMATIC_GRAPHICS_SWITCHING,
            0,
        ],
        // Without hardware acceleration (virtual machines).
        &[
            PFA_OPENGL_PROFILE,
            OPENGL_PROFILE_3_2_CORE,
            PFA_DOUBLE_BUFFER,
            0,
        ],
    ];
    for attributes in attempts {
        let mut pixel_format: CGLPixelFormatObj = ptr::null_mut();
        let mut count: c_int = 0;
        let error =
            unsafe { CGLChoosePixelFormat(attributes.as_ptr(), &mut pixel_format, &mut count) };
        if error != 0 || pixel_format.is_null() {
            continue;
        }
        let mut context: CGLContextObj = ptr::null_mut();
        let error = unsafe { CGLCreateContext(pixel_format, ptr::null_mut(), &mut context) };
        if error != 0 || context.is_null() {
            unsafe { CGLReleasePixelFormat(pixel_format) };
            continue;
        }
        return Ok((context, pixel_format));
    }
    bail!("no OpenGL pixel format available")
}

fn create_render_context(mpv: &Mpv, cgl: CGLContextObj) -> Result<*mut sys::mpv_render_context> {
    let mut init = sys::mpv_opengl_init_params {
        get_proc_address: Some(get_proc_address),
        get_proc_address_ctx: ptr::null_mut(),
    };
    let mut params = [
        sys::mpv_render_param {
            type_: sys::mpv_render_param_type_MPV_RENDER_PARAM_API_TYPE,
            data: sys::MPV_RENDER_API_TYPE_OPENGL.as_ptr() as *mut c_void,
        },
        sys::mpv_render_param {
            type_: sys::mpv_render_param_type_MPV_RENDER_PARAM_OPENGL_INIT_PARAMS,
            data: (&raw mut init).cast(),
        },
        sys::mpv_render_param {
            type_: 0,
            data: ptr::null_mut(),
        },
    ];
    let mut render: *mut sys::mpv_render_context = ptr::null_mut();
    let error = unsafe {
        let previous = CGLGetCurrentContext();
        CGLSetCurrentContext(cgl);
        let error =
            sys::mpv_render_context_create(&mut render, mpv.ctx.as_ptr(), params.as_mut_ptr());
        CGLSetCurrentContext(previous);
        error
    };
    if error < 0 || render.is_null() {
        bail!("creating mpv's OpenGL renderer failed ({error})");
    }
    Ok(render)
}

fn gpui_view(window: &Window) -> Result<Id> {
    let handle = HasWindowHandle::window_handle(window).map_err(|e| anyhow!("{e}"))?;
    match handle.as_raw() {
        RawWindowHandle::AppKit(h) => Ok(h.ns_view.as_ptr() as Id),
        other => Err(anyhow!("unsupported window handle: {other:?}")),
    }
}

/// The view that shows the video, below GPUI's view.
pub struct Host {
    view: Id,
    layer: Id,
    renderer: Arc<Renderer>,
}

impl Host {
    /// mpv options for rendering through the render API.
    pub fn configure(init: &MpvInitializer) -> libmpv2::Result<()> {
        init.set_option("vo", "libmpv")?;
        Ok(())
    }

    pub fn new(window: &Window, mpv: &Mpv) -> Result<Self> {
        let gpui_view = gpui_view(window)?;
        let (cgl, pixel_format) = create_gl_context()?;
        let renderer = Arc::new(Renderer {
            cgl,
            pixel_format,
            render: Mutex::new(RenderContext(ptr::null_mut())),
            update: AtomicBool::new(false),
            redraw: AtomicBool::new(true),
        });
        let render = create_render_context(mpv, cgl)?;
        renderer.render.lock().unwrap().0 = render;
        unsafe {
            sys::mpv_render_context_set_update_callback(
                render,
                Some(on_update),
                Arc::as_ptr(&renderer) as *mut c_void,
            );
        }

        unsafe {
            let superview: Id = msg_send![gpui_view, superview];
            if superview.is_null() {
                renderer.free();
                bail!("the window has no content view");
            }
            let layer: Id = msg_send![layer_class(), new];
            (*layer).set_ivar(
                RENDERER_IVAR,
                Arc::into_raw(renderer.clone()) as *const c_void,
            );
            let _: () = msg_send![layer, setAsynchronous: YES];
            let _: () = msg_send![layer, setNeedsDisplayOnBoundsChange: YES];
            let _: () = msg_send![layer, setOpaque: YES];

            let frame: NSRect = msg_send![superview, bounds];
            let view: Id = msg_send![class!(NSView), alloc];
            let view: Id = msg_send![view, initWithFrame: frame];
            let _: () = msg_send![view, setLayer: layer];
            let _: () = msg_send![view, setWantsLayer: YES];
            let _: () = msg_send![view, setAutoresizingMask: NS_VIEW_WIDTH_SIZABLE | NS_VIEW_HEIGHT_SIZABLE];
            let _: () = msg_send![superview,
                addSubview: view
                positioned: NS_WINDOW_BELOW
                relativeTo: gpui_view];
            let host = Self {
                view,
                layer,
                renderer,
            };
            host.sync_size();
            Ok(host)
        }
    }

    /// The view follows the window by itself; this keeps the layer's pixel
    /// density in step with the screen and redraws.
    pub fn sync_size(&self) {
        unsafe {
            let window: Id = msg_send![self.view, window];
            if !window.is_null() {
                let scale: f64 = msg_send![window, backingScaleFactor];
                let _: () = msg_send![self.layer, setContentsScale: scale];
            }
        }
        self.renderer.redraw.store(true, Ordering::Release);
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        self.renderer.free();
        unsafe {
            let _: () = msg_send![self.view, removeFromSuperview];
            let _: () = msg_send![self.view, release];
            let _: () = msg_send![self.layer, release];
        }
    }
}
