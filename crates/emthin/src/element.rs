use smithay::{
    backend::renderer::{
        element::{
            memory::MemoryRenderBufferRenderElement, solid::SolidColorRenderElement,
            surface::WaylandSurfaceRenderElement, texture::TextureRenderElement,
        },
        gles::GlesTexture,
        ImportAll, ImportMem, Renderer,
    },
    render_elements,
};

pub trait EmthinRenderer: ImportAll + ImportMem + Renderer<TextureId = GlesTexture> {}
impl<R: ImportAll + ImportMem + Renderer<TextureId = GlesTexture>> EmthinRenderer for R {}

render_elements! {
    pub CustomElement<R> where R: EmthinRenderer;
    Surface=WaylandSurfaceRenderElement<R>,
    Mirror=TextureRenderElement<GlesTexture>,
    Solid=SolidColorRenderElement,
    Label=MemoryRenderBufferRenderElement<R>,
}
