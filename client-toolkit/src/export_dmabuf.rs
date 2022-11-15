use cosmic_protocols::export_dmabuf::v1::client::{
    zcosmic_export_dmabuf_frame_v1, zcosmic_export_dmabuf_manager_v1,
};
use sctk::registry::{ProvidesRegistryState, RegistryState};
#[cfg(feature = "smithay")]
use smithay::{
    backend::{
        allocator::{
            dmabuf::{Dmabuf, DmabufFlags},
            Fourcc, Modifier,
        },
        drm::node::DrmNode,
        renderer::{
            gles2::{Gles2Renderer, Gles2Texture},
            multigpu::{egl::EglGlesBackend, GpuManager},
            Bind, ExportMem,
        },
    },
    utils::{Point, Rectangle, Size},
};
use std::{collections::HashMap, os::unix::io::OwnedFd};
use wayland_client::{backend::ObjectId, Connection, Dispatch, Proxy, QueueHandle};

#[derive(Debug)]
pub struct Object {
    pub fd: OwnedFd,
    pub index: u32,
    pub offset: u32,
    pub stride: u32,
    pub plane_index: u32,
}

#[derive(Debug, Default)]
pub struct DmabufFrame {
    pub node: u64,
    pub width: u32,
    pub height: u32,
    pub objects: Vec<Object>,
    pub modifier: u64,
    pub format: u32,
    pub flags: u32,
}

#[cfg(feature = "smithay")]
impl DmabufFrame {
    // TODO: Don't create new renderer every frame?
    pub fn import_to_bytes(
        self,
        gpu_manager: &mut GpuManager<EglGlesBackend<Gles2Renderer>>,
    ) -> Vec<u8> {
        let mut builder = Dmabuf::builder(
            (self.width as i32, self.height as i32),
            Fourcc::try_from(self.format).unwrap(),
            DmabufFlags::from_bits(u32::from(self.flags)).unwrap(),
        );
        for object in self.objects {
            builder.add_plane(
                object.fd,
                object.index,
                object.offset,
                object.stride,
                Modifier::from(self.modifier),
            );
        }
        let dmabuf = builder.build().unwrap();

        let drm_node = DrmNode::from_dev_id(self.node).unwrap();
        let mut renderer = gpu_manager
            .renderer::<Gles2Texture>(&drm_node, &drm_node)
            .unwrap();
        renderer.bind(dmabuf).unwrap();
        let rectangle = Rectangle {
            loc: Point::default(),
            size: Size::from((self.width as i32, self.height as i32)),
        };
        let mapping = renderer.copy_framebuffer(rectangle).unwrap();
        Vec::from(renderer.map_texture(&mapping).unwrap())
    }
}

pub struct ExportDmabufState {
    export_dmabuf_manager: Option<zcosmic_export_dmabuf_manager_v1::ZcosmicExportDmabufManagerV1>,
    frames: HashMap<ObjectId, DmabufFrame>,
}

impl ExportDmabufState {
    pub fn new<D>(registry: &RegistryState, qh: &QueueHandle<D>) -> Self
    where
        D: Dispatch<zcosmic_export_dmabuf_manager_v1::ZcosmicExportDmabufManagerV1, ()> + 'static,
    {
        let export_dmabuf_manager = registry.bind_one(&qh, 1..=1, ()).ok();

        Self {
            export_dmabuf_manager,
            frames: HashMap::new(),
        }
    }

    pub fn export_dmabuf_manager(
        &self,
    ) -> Option<&zcosmic_export_dmabuf_manager_v1::ZcosmicExportDmabufManagerV1> {
        self.export_dmabuf_manager.as_ref()
    }
}

pub trait ExportDmabufHandler {
    fn export_dmabuf_state(&mut self) -> &mut ExportDmabufState;

    fn frame_ready(
        &mut self,
        frame: &zcosmic_export_dmabuf_frame_v1::ZcosmicExportDmabufFrameV1,
        dmabuf: DmabufFrame,
    );

    fn frame_cancel(&mut self, frame: &zcosmic_export_dmabuf_frame_v1::ZcosmicExportDmabufFrameV1);
}

impl<D> Dispatch<zcosmic_export_dmabuf_manager_v1::ZcosmicExportDmabufManagerV1, (), D>
    for ExportDmabufState
where
    D: Dispatch<zcosmic_export_dmabuf_manager_v1::ZcosmicExportDmabufManagerV1, ()>,
{
    fn event(
        _: &mut D,
        _: &zcosmic_export_dmabuf_manager_v1::ZcosmicExportDmabufManagerV1,
        _: zcosmic_export_dmabuf_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<D>,
    ) {
    }
}

impl<D> Dispatch<zcosmic_export_dmabuf_frame_v1::ZcosmicExportDmabufFrameV1, (), D>
    for ExportDmabufState
where
    D: Dispatch<zcosmic_export_dmabuf_frame_v1::ZcosmicExportDmabufFrameV1, ()>
        + ExportDmabufHandler,
{
    fn event(
        state: &mut D,
        frame: &zcosmic_export_dmabuf_frame_v1::ZcosmicExportDmabufFrameV1,
        event: zcosmic_export_dmabuf_frame_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<D>,
    ) {
        match event {
            zcosmic_export_dmabuf_frame_v1::Event::Device { ref node } => {
                let dmabuf = state
                    .export_dmabuf_state()
                    .frames
                    .entry(frame.id())
                    .or_default();
                dmabuf.node = u64::from_ne_bytes([
                    node[0], node[1], node[2], node[3], node[4], node[5], node[6], node[7],
                ]);
            }
            zcosmic_export_dmabuf_frame_v1::Event::Frame {
                width,
                height,
                mod_high,
                mod_low,
                format,
                flags,
                ..
            } => {
                let dmabuf = state
                    .export_dmabuf_state()
                    .frames
                    .entry(frame.id())
                    .or_default();
                dmabuf.width = width;
                dmabuf.height = height;
                dmabuf.modifier = ((mod_high as u64) << 32) + mod_low as u64;
                dmabuf.format = format;
                dmabuf.flags = u32::from(flags);
            }
            zcosmic_export_dmabuf_frame_v1::Event::Object {
                fd,
                index,
                offset,
                stride,
                plane_index,
                ..
            } => {
                let dmabuf = state
                    .export_dmabuf_state()
                    .frames
                    .entry(frame.id())
                    .or_default();
                dmabuf.objects.push(Object {
                    fd,
                    index,
                    offset,
                    stride,
                    plane_index,
                });
            }
            zcosmic_export_dmabuf_frame_v1::Event::Ready { .. } => {
                let dmabuf = state
                    .export_dmabuf_state()
                    .frames
                    .remove(&frame.id())
                    .unwrap();
                state.frame_ready(frame, dmabuf);
            }
            zcosmic_export_dmabuf_frame_v1::Event::Cancel { .. } => {
                state.export_dmabuf_state().frames.remove(&frame.id());
                state.frame_cancel(frame);
            }
            _ => {}
        }
    }
}

#[macro_export]
macro_rules! delegate_export_dmabuf {
    ($ty: ty) => {
        $crate::wayland_client::delegate_dispatch!($ty: [
            $crate::cosmic_protocols::export_dmabuf::v1::client::zcosmic_export_dmabuf_manager_v1::ZcosmicExportDmabufManagerV1: ()
        ] => $crate::export_dmabuf::ExportDmabufState);
        $crate::wayland_client::delegate_dispatch!($ty: [
            $crate::cosmic_protocols::export_dmabuf::v1::client::zcosmic_export_dmabuf_frame_v1::ZcosmicExportDmabufFrameV1: ()
        ] => $crate::export_dmabuf::ExportDmabufState);
    };
}
