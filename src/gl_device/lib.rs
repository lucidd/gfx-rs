// Copyright 2014 The Gfx-rs Developers.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! OpenGL implementation of a device, striving to support OpenGL 2.0 with at
//! least VAOs, but using newer extensions when available.

#![allow(missing_doc)]
#![experimental]

extern crate libc;
extern crate "gfx_gl" as gl;

use log;

use attrib;

use Device;

pub use self::draw::GlCommandBuffer;
pub use self::info::{Info, PlatformName, Version};

mod draw;
mod shade;
mod state;
mod tex;
mod info;

pub type Buffer         = gl::types::GLuint;
pub type ArrayBuffer    = gl::types::GLuint;
pub type Shader         = gl::types::GLuint;
pub type Program        = gl::types::GLuint;
pub type FrameBuffer    = gl::types::GLuint;
pub type Surface        = gl::types::GLuint;
pub type Sampler        = gl::types::GLuint;
pub type Texture        = gl::types::GLuint;

#[deriving(Eq, PartialEq, Show)]
pub enum GlError {
    NoError,
    InvalidEnum,
    InvalidValue,
    InvalidOperation,
    InvalidFramebufferOperation,
    OutOfMemory,
    UnknownError,
}

impl GlError {
    pub fn from_error_code(error_code: gl::types::GLenum) -> GlError {
        match error_code {
            gl::NO_ERROR                      => NoError,
            gl::INVALID_ENUM                  => InvalidEnum,
            gl::INVALID_VALUE                 => InvalidValue,
            gl::INVALID_OPERATION             => InvalidOperation,
            gl::INVALID_FRAMEBUFFER_OPERATION => InvalidFramebufferOperation,
            gl::OUT_OF_MEMORY                 => OutOfMemory,
            _                                 => UnknownError,
        }
    }
}

static RESET_CB: &'static [::Command] = &[
    ::BindProgram(0),
    ::BindArrayBuffer(0),
    //BindAttribute
    ::BindIndex(0),
    ::BindFrameBuffer(::target::Draw, 0),
    ::BindFrameBuffer(::target::Read, 0),
    //UnbindTarget
    //BindUniformBlock
    //BindUniform
    //BindTexture
    ::SetPrimitiveState(::state::Primitive {
        front_face: ::state::CounterClockwise,
        method: ::state::Fill(::state::CullNothing),
        offset: ::state::NoOffset,
    }),
    ::SetViewport(::target::Rect{x: 0, y: 0, w: 0, h: 0}),
    ::SetScissor(None),
    ::SetDepthStencilState(None, None, ::state::CullNothing),
    ::SetBlendState(None),
    ::SetColorMask(::state::MASK_ALL),
];

fn primitive_to_gl(prim_type: ::PrimitiveType) -> gl::types::GLenum {
    match prim_type {
        ::Point => gl::POINTS,
        ::Line => gl::LINES,
        ::LineStrip => gl::LINE_STRIP,
        ::TriangleList => gl::TRIANGLES,
        ::TriangleStrip => gl::TRIANGLE_STRIP,
        ::TriangleFan => gl::TRIANGLE_FAN,
    }
}

fn access_to_gl(access: ::target::Access) -> gl::types::GLenum {
    match access {
        ::target::Draw => gl::DRAW_FRAMEBUFFER,
        ::target::Read => gl::READ_FRAMEBUFFER,
    }
}

fn target_to_gl(target: ::target::Target) -> gl::types::GLenum {
    match target {
        ::target::TargetColor(index) =>
            gl::COLOR_ATTACHMENT0 + (index as gl::types::GLenum),
        ::target::TargetDepth => gl::DEPTH_ATTACHMENT,
        ::target::TargetStencil => gl::STENCIL_ATTACHMENT,
        ::target::TargetDepthStencil => gl::DEPTH_STENCIL_ATTACHMENT,
    }
}

/// An OpenGL device with GLSL shaders
pub struct GlDevice {
    info: Info,
    caps: ::Capabilities,
    gl: gl::Gl,
}

impl GlDevice {
    /// Load OpenGL symbols and detect driver information
    pub fn new(fn_proc: |&str| -> *const ::libc::c_void) -> GlDevice {
        let gl = gl::Gl::load_with(fn_proc);

        let (info, caps) = info::get(&gl);

        info!("Vendor: {}", info.platform_name.vendor);
        info!("Renderer: {}", info.platform_name.renderer);
        info!("Version: {}", info.version);
        info!("Shading Language: {}", info.shading_language);
        info!("Loaded Extensions:");
        for extension in info.extensions.iter() {
            info!("- {}", *extension);
        }

        GlDevice {
            info: info,
            caps: caps,
            gl: gl,
        }
    }

    /// Access the OpenGL directly via a closure. OpenGL types and enumerations
    /// can be found in the `gl` crate.
    pub unsafe fn with_gl(&mut self, fun: |&gl::Gl|) {
        self.reset_state();
        fun(&self.gl);
    }

    /// Fails during a debug build if the implementation's error flag was set.
    fn check(&mut self, cmd: &::Command) {
        if cfg!(not(ndebug)) {
            let err = GlError::from_error_code(self.gl.GetError());
            if err != NoError {
                panic!("Error after executing command {}: {}", cmd, err);
            }
        }
    }

    /// Get the OpenGL-specific driver information
    pub fn get_info<'a>(&'a self) -> &'a Info {
        &self.info
    }

    fn create_buffer_internal(&mut self) -> Buffer {
        let mut name = 0 as Buffer;
        unsafe {
            self.gl.GenBuffers(1, &mut name);
        }
        info!("\tCreated buffer {}", name);
        name
    }

    fn init_buffer(&mut self, buffer: Buffer, info: &::BufferInfo) {
        self.gl.BindBuffer(gl::ARRAY_BUFFER, buffer);
        let usage = match info.usage {
            ::UsageStatic  => gl::STATIC_DRAW,
            ::UsageDynamic => gl::DYNAMIC_DRAW,
            ::UsageStream  => gl::STREAM_DRAW,
        };
        unsafe {
            self.gl.BufferData(gl::ARRAY_BUFFER,
                info.size as gl::types::GLsizeiptr,
                0 as *const gl::types::GLvoid,
                usage
            );
        }
    }

    fn update_sub_buffer(&mut self, buffer: Buffer, address: *const u8,
                         size: uint, offset: uint) {
        self.gl.BindBuffer(gl::ARRAY_BUFFER, buffer);
        unsafe {
            self.gl.BufferSubData(gl::ARRAY_BUFFER,
                offset as gl::types::GLintptr,
                size as gl::types::GLsizeiptr,
                address as *const gl::types::GLvoid
            );
        }
    }

    fn process(&mut self, cmd: &::Command, data_buf: &::draw::DataBuffer) {
        match *cmd {
            ::Clear(ref data, mask) => {
                let mut flags = 0;
                if mask.intersects(::target::COLOR) {
                    flags |= gl::COLOR_BUFFER_BIT;
                    state::bind_color_mask(&self.gl, ::state::MASK_ALL);
                    let [r, g, b, a] = data.color;
                    self.gl.ClearColor(r, g, b, a);
                }
                if mask.intersects(::target::DEPTH) {
                    flags |= gl::DEPTH_BUFFER_BIT;
                    self.gl.DepthMask(gl::TRUE);
                    self.gl.ClearDepth(data.depth as gl::types::GLclampd);
                }
                if mask.intersects(::target::STENCIL) {
                    flags |= gl::STENCIL_BUFFER_BIT;
                    self.gl.StencilMask(-1);
                    self.gl.ClearStencil(data.stencil as gl::types::GLint);
                }
                self.gl.Clear(flags);
            },
            ::BindProgram(program) => {
                self.gl.UseProgram(program);
            },
            ::BindArrayBuffer(array_buffer) => {
                if self.caps.array_buffer_supported {
                    self.gl.BindVertexArray(array_buffer);
                } else {
                    error!("Ignored VAO bind command: {}", array_buffer)
                }
            },
            ::BindAttribute(slot, buffer, format) => {
                let gl_type = match format.elem_type {
                    attrib::Int(_, attrib::U8, attrib::Unsigned)  => gl::UNSIGNED_BYTE,
                    attrib::Int(_, attrib::U8, attrib::Signed)    => gl::BYTE,
                    attrib::Int(_, attrib::U16, attrib::Unsigned) => gl::UNSIGNED_SHORT,
                    attrib::Int(_, attrib::U16, attrib::Signed)   => gl::SHORT,
                    attrib::Int(_, attrib::U32, attrib::Unsigned) => gl::UNSIGNED_INT,
                    attrib::Int(_, attrib::U32, attrib::Signed)   => gl::INT,
                    attrib::Float(_, attrib::F16) => gl::HALF_FLOAT,
                    attrib::Float(_, attrib::F32) => gl::FLOAT,
                    attrib::Float(_, attrib::F64) => gl::DOUBLE,
                    _ => {
                        error!("Unsupported element type: {}", format.elem_type);
                        return
                    }
                };
                self.gl.BindBuffer(gl::ARRAY_BUFFER, buffer);
                let offset = format.offset as *const gl::types::GLvoid;
                match format.elem_type {
                    attrib::Int(attrib::IntRaw, _, _) => unsafe {
                        self.gl.VertexAttribIPointer(slot as gl::types::GLuint,
                            format.elem_count as gl::types::GLint, gl_type,
                            format.stride as gl::types::GLint, offset);
                    },
                    attrib::Int(attrib::IntNormalized, _, _) => unsafe {
                        self.gl.VertexAttribPointer(slot as gl::types::GLuint,
                            format.elem_count as gl::types::GLint, gl_type, gl::TRUE,
                            format.stride as gl::types::GLint, offset);
                    },
                    attrib::Int(attrib::IntAsFloat, _, _) => unsafe {
                        self.gl.VertexAttribPointer(slot as gl::types::GLuint,
                            format.elem_count as gl::types::GLint, gl_type, gl::FALSE,
                            format.stride as gl::types::GLint, offset);
                    },
                    attrib::Float(attrib::FloatDefault, _) => unsafe {
                        self.gl.VertexAttribPointer(slot as gl::types::GLuint,
                            format.elem_count as gl::types::GLint, gl_type, gl::FALSE,
                            format.stride as gl::types::GLint, offset);
                    },
                    attrib::Float(attrib::FloatPrecision, _) => unsafe {
                        self.gl.VertexAttribLPointer(slot as gl::types::GLuint,
                            format.elem_count as gl::types::GLint, gl_type,
                            format.stride as gl::types::GLint, offset);
                    },
                    _ => ()
                }
                self.gl.EnableVertexAttribArray(slot as gl::types::GLuint);
                if self.caps.instance_rate_supported {
                    self.gl.VertexAttribDivisor(slot as gl::types::GLuint,
                        format.instance_rate as gl::types::GLuint);
                }else if format.instance_rate != 0 {
                    error!("Instanced arrays are not supported");
                }
            },
            ::BindIndex(buffer) => {
                self.gl.BindBuffer(gl::ELEMENT_ARRAY_BUFFER, buffer);
            },
            ::BindFrameBuffer(access, frame_buffer) => {
                let point = access_to_gl(access);
                self.gl.BindFramebuffer(point, frame_buffer);
            },
            ::UnbindTarget(access, target) => {
                let point = access_to_gl(access);
                let att = target_to_gl(target);
                self.gl.FramebufferRenderbuffer(point, att, gl::RENDERBUFFER, 0);
            },
            ::BindTargetSurface(access, target, name) => {
                let point = access_to_gl(access);
                let att = target_to_gl(target);
                self.gl.FramebufferRenderbuffer(point, att, gl::RENDERBUFFER, name);
            },
            ::BindTargetTexture(access, target, name, level, layer) => {
                let point = access_to_gl(access);
                let att = target_to_gl(target);
                match layer {
                    Some(layer) => self.gl.FramebufferTextureLayer(
                        point, att, name, level as gl::types::GLint,
                        layer as gl::types::GLint),
                    None => self.gl.FramebufferTexture(
                        point, att, name, level as gl::types::GLint),
                }
            },
            ::BindUniformBlock(program, slot, loc, buffer) => {
                self.gl.UniformBlockBinding(program, slot as gl::types::GLuint, loc as gl::types::GLuint);
                self.gl.BindBufferBase(gl::UNIFORM_BUFFER, loc as gl::types::GLuint, buffer);
            },
            ::BindUniform(loc, uniform) => {
                shade::bind_uniform(&self.gl, loc as gl::types::GLint, uniform);
            },
            ::BindTexture(slot, kind, texture, sampler) => {
                let anchor = tex::bind_texture(&self.gl,
                    gl::TEXTURE0 + slot as gl::types::GLenum,
                    kind, texture);
                match (anchor, kind.get_aa_mode(), sampler) {
                    (Err(e), _, _) => error!("Invalid texture bind: {}", e),
                    (Ok(anchor), None, Some(::Handle(sam, ref info))) => {
                        if self.caps.sampler_objects_supported {
                            self.gl.BindSampler(slot as gl::types::GLenum, sam);
                        } else {
                            debug_assert_eq!(sam, 0);
                            tex::bind_sampler(&self.gl, anchor, info);
                        }
                    },
                    (Ok(_), Some(_), Some(_)) =>
                        error!("Unable to bind a multi-sampled texture with a sampler"),
                    (Ok(_), _, _) => (),
                }
            },
            ::SetPrimitiveState(prim) => {
                state::bind_primitive(&self.gl, prim);
            },
            ::SetViewport(rect) => {
                state::bind_viewport(&self.gl, rect);
            },
            ::SetMultiSampleState(ms) => {
                state::bind_multi_sample(&self.gl, ms);
            },
            ::SetScissor(rect) => {
                state::bind_scissor(&self.gl, rect);
            },
            ::SetDepthStencilState(depth, stencil, cull) => {
                state::bind_stencil(&self.gl, stencil, cull);
                state::bind_depth(&self.gl, depth);
            },
            ::SetBlendState(blend) => {
                state::bind_blend(&self.gl, blend);
            },
            ::SetColorMask(mask) => {
                state::bind_color_mask(&self.gl, mask);
            },
            ::UpdateBuffer(buffer, pointer, offset) => {
                let data = data_buf.get_ref(pointer);
                self.update_sub_buffer(buffer, data.as_ptr(), data.len(), offset);
            },
            ::UpdateTexture(kind, texture, image_info, pointer) => {
                let data = data_buf.get_ref(pointer);
                match tex::update_texture(&self.gl, kind, texture, &image_info,
                                          data.as_ptr(), data.len()) {
                    Ok(_) => (),
                    Err(_) => unimplemented!(),
                }
            },
            ::Draw(prim_type, start, count, instances) => {
                match instances {
                    Some(num) if self.caps.instance_call_supported => {
                        self.gl.DrawArraysInstanced(
                            primitive_to_gl(prim_type),
                            start as gl::types::GLsizei,
                            count as gl::types::GLsizei,
                            num as gl::types::GLsizei
                        );
                    },
                    Some(_) => {
                        error!("Instanced draw calls are not supported");
                    },
                    None => {
                        self.gl.DrawArrays(
                            primitive_to_gl(prim_type),
                            start as gl::types::GLsizei,
                            count as gl::types::GLsizei
                        );
                    },
                }
            },
            ::DrawIndexed(prim_type, index_type, start, count, instances) => {
                let (offset, gl_index) = match index_type {
                    attrib::U8  => (start * 1u32, gl::UNSIGNED_BYTE),
                    attrib::U16 => (start * 2u32, gl::UNSIGNED_SHORT),
                    attrib::U32 => (start * 4u32, gl::UNSIGNED_INT),
                };
                match instances {
                    Some(num) if self.caps.instance_call_supported => unsafe {
                        self.gl.DrawElementsInstanced(
                            primitive_to_gl(prim_type),
                            count as gl::types::GLsizei,
                            gl_index,
                            offset as *const gl::types::GLvoid,
                            num as gl::types::GLsizei
                        );
                    },
                    Some(_) => {
                        error!("Instanced draw calls are not supported");
                    },
                    None => unsafe {
                        self.gl.DrawElements(
                            primitive_to_gl(prim_type),
                            count as gl::types::GLsizei,
                            gl_index,
                            offset as *const gl::types::GLvoid
                        );
                    },
                }
            },
            ::Blit(s_rect, d_rect, mask) => {
                type GLint = gl::types::GLint;
                // build mask
                let mut flags = 0;
                if mask.intersects(::target::COLOR) {
                    flags |= gl::COLOR_BUFFER_BIT;
                }
                if mask.intersects(::target::DEPTH) {
                    flags |= gl::DEPTH_BUFFER_BIT;
                }
                if mask.intersects(::target::STENCIL) {
                    flags |= gl::STENCIL_BUFFER_BIT;
                }
                // build filter
                let filter = if s_rect.w == d_rect.w && s_rect.h == d_rect.h {
                    gl::NEAREST
                }else {
                    gl::LINEAR
                };
                // blit
                self.gl.BlitFramebuffer(
                    s_rect.x as GLint,
                    s_rect.y as GLint,
                    (s_rect.x + s_rect.w) as GLint,
                    (s_rect.y + s_rect.h) as GLint,
                    d_rect.x as GLint,
                    d_rect.y as GLint,
                    (d_rect.x + d_rect.w) as GLint,
                    (d_rect.y + d_rect.h) as GLint,
                    flags,
                    filter
                );
            },
        }
        self.check(cmd);
    }
}

impl Device<GlCommandBuffer> for GlDevice {
    fn get_capabilities<'a>(&'a self) -> &'a ::Capabilities {
        &self.caps
    }

    fn reset_state(&mut self) {
        let data = ::draw::DataBuffer::new();
        for com in RESET_CB.iter() {
            self.process(com, &data);
        }
    }

    fn submit(&mut self, (cb, db): (&GlCommandBuffer, &::draw::DataBuffer)) {
        self.reset_state();
        for com in cb.iter() {
            self.process(com, db);
        }
    }

    fn create_buffer_raw(&mut self, size: uint, usage: ::BufferUsage) -> ::BufferHandle<()> {
        let name = self.create_buffer_internal();
        let info = ::BufferInfo {
            usage: usage,
            size: size,
        };
        self.init_buffer(name, &info);
        ::BufferHandle::from_raw(::Handle(name, info))
    }

    fn create_buffer_static_raw(&mut self, data: &[u8]) -> ::BufferHandle<()> {
        let name = self.create_buffer_internal();

        let info = ::BufferInfo {
            usage: ::UsageStatic,
            size: data.len(),
        };
        self.init_buffer(name, &info);
        self.update_sub_buffer(name, data.as_ptr(), data.len(), 0);
        ::BufferHandle::from_raw(::Handle(name, info))
    }

    fn create_array_buffer(&mut self) -> Result<::ArrayBufferHandle, ()> {
        if self.caps.array_buffer_supported {
            let mut name = 0 as ArrayBuffer;
            unsafe {
                self.gl.GenVertexArrays(1, &mut name);
            }
            info!("\tCreated array buffer {}", name);
            Ok(::Handle(name, ()))
        } else {
            error!("\tarray buffer creation unsupported, ignored")
            Err(())
        }
    }

    fn create_shader(&mut self, stage: ::shade::Stage, code: ::shade::ShaderSource)
                     -> Result<::ShaderHandle, ::shade::CreateShaderError> {
        let (name, info) = shade::create_shader(&self.gl, stage, code, self.info.shading_language);
        info.map(|info| {
            let level = if name.is_err() { log::ERROR } else { log::WARN };
            log!(level, "\tShader compile log: {}", info);
        });
        name.map(|sh| ::Handle(sh, stage))
    }

    fn create_program(&mut self, shaders: &[::ShaderHandle]) -> Result<::ProgramHandle, ()> {
        let (prog, log) = shade::create_program(&self.gl, &self.caps, shaders);
        log.map(|log| {
            let level = if prog.is_err() { log::ERROR } else { log::WARN };
            log!(level, "\tProgram link log: {}", log);
        });
        prog
    }

    fn create_frame_buffer(&mut self) -> ::FrameBufferHandle {
        let mut name = 0 as FrameBuffer;
        unsafe {
            self.gl.GenFramebuffers(1, &mut name);
        }
        info!("\tCreated frame buffer {}", name);
        ::Handle(name, ())
    }

    fn create_surface(&mut self, info: ::tex::SurfaceInfo) ->
                      Result<::SurfaceHandle, ::tex::SurfaceError> {
        tex::make_surface(&self.gl, &info).map(|suf| ::Handle(suf, info))
    }

    fn create_texture(&mut self, info: ::tex::TextureInfo) ->
                      Result<::TextureHandle, ::tex::TextureError> {

        if info.width == 0 || info.height == 0 || info.levels == 0 {
            return Err(::tex::InvalidTextureInfo(info))
        }

        let name = if self.caps.immutable_storage_supported {
            tex::make_with_storage(&self.gl, &info)
        } else {
            tex::make_without_storage(&self.gl, &info)
        };
        name.map(|tex| ::Handle(tex, info))
    }

    fn create_sampler(&mut self, info: ::tex::SamplerInfo) -> ::SamplerHandle {
        let sam = if self.caps.sampler_objects_supported {
            tex::make_sampler(&self.gl, &info)
        } else {
            0
        };
        ::Handle(sam, info)
    }

    fn delete_buffer_raw(&mut self, handle: ::BufferHandle<()>) {
        let name = handle.get_name();
        unsafe {
            self.gl.DeleteBuffers(1, &name);
        }
    }

    fn delete_shader(&mut self, handle: ::ShaderHandle) {
        self.gl.DeleteShader(handle.get_name());
    }

    fn delete_program(&mut self, handle: ::ProgramHandle) {
        self.gl.DeleteProgram(handle.get_name());
    }

    fn delete_surface(&mut self, handle: ::SurfaceHandle) {
        let name = handle.get_name();
        unsafe {
            self.gl.DeleteRenderbuffers(1, &name);
        }
    }

    fn delete_texture(&mut self, handle: ::TextureHandle) {
        let name = handle.get_name();
        unsafe {
            self.gl.DeleteTextures(1, &name);
        }
    }

    fn delete_sampler(&mut self, handle: ::SamplerHandle) {
        let name = handle.get_name();
        unsafe {
            self.gl.DeleteSamplers(1, &name);
        }
    }

    fn update_buffer_raw(&mut self, buffer: ::BufferHandle<()>, data: &[u8],
                         offset_bytes: uint) {
        debug_assert!(offset_bytes + data.len() <= buffer.get_info().size);
        self.update_sub_buffer(buffer.get_name(), data.as_ptr(), data.len(),
                               offset_bytes)
    }

    fn update_texture_raw(&mut self, texture: &::TextureHandle,
                          img: &::tex::ImageInfo, data: &[u8])
                          -> Result<(), ::tex::TextureError> {
        tex::update_texture(&self.gl, texture.get_info().kind,
                            texture.get_name(), img, data.as_ptr(), data.len())
    }

    fn generate_mipmap(&mut self, texture: &::TextureHandle) {
        tex::generate_mipmap(&self.gl, texture.get_info().kind, texture.get_name());
    }
}
