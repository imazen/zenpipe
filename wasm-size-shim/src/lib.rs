//! WASM size shim — forces LTO to keep all zenpipe + all codec code paths.
extern crate alloc;
use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::vec::Vec;
use zenpipe::*;
use zenpipe::graph::*;
use zenpipe::sources::*;
use zenpipe::format;
use zencodec::decode::DynDecoderConfig;
use zencodec::encode::DynEncoderConfig;

fn drain(mut src: Box<dyn Source>) -> Vec<u8> {
    let mut out = Vec::new();
    while let Ok(Some(strip)) = src.next() { out.extend_from_slice(strip.as_strided_bytes()); }
    out
}
fn msrc(nodes: &[NodeId], dims: &[(u32,u32)], fmts: &[PixelFormat], bufs: &[&[u8]]) -> hashbrown::HashMap<NodeId, Box<dyn Source>> {
    let mut m = hashbrown::HashMap::new();
    for (i, &n) in nodes.iter().enumerate() { m.insert(n, Box::new(CallbackSource::from_data(bufs[i],dims[i].0,dims[i].1,fmts[i],16)) as Box<dyn Source>); }
    m
}
fn ret(v: Vec<u8>) -> *mut u8 { if v.is_empty(){return core::ptr::null_mut()} let p=v.as_ptr() as *mut u8; core::mem::forget(v); p }

/// Generic streaming decode via zencodec DynDecoderConfig
fn generic_decode(config: &dyn DynDecoderConfig, data: &[u8], ow: *mut u32, oh: *mut u32) -> *mut u8 {
    let job = config.dyn_job();
    let pref = [zenpixels::PixelDescriptor::RGBA8_SRGB];
    let Ok(dec) = job.into_streaming_decoder(Cow::Owned(data.to_vec()), &pref) else { return core::ptr::null_mut() };
    let info = dec.info();
    unsafe { *ow = info.width; *oh = info.height; }
    let mut out = Vec::new();
    let mut decoder = dec;
    loop {
        match decoder.next_batch() {
            Ok(Some((_y, pixels))) => out.extend_from_slice(pixels.as_strided_bytes()),
            _ => break,
        }
    }
    ret(out)
}

// === Pipeline ops ===

#[unsafe(no_mangle)] pub extern "C" fn op_resize(sp:*const u8,sl:u32,sw:u32,sh:u32,dw:u32,dh:u32,fi:u32)->*mut u8{
    let s=unsafe{core::slice::from_raw_parts(sp,sl as usize)}; let f=format::RGBA8_SRGB;
    let mut g=PipelineGraph::new(); let sn=g.add_node(NodeOp::Source);
    let rn=g.add_node(NodeOp::Resize{w:dw,h:dh,filter:Some(match fi{1=>zenresize::Filter::Lanczos,2=>zenresize::Filter::CatmullRom,_=>zenresize::Filter::Robidoux}),sharpen_percent:None});
    let on=g.add_node(NodeOp::Output); g.add_edge(sn,rn,EdgeKind::Input); g.add_edge(rn,on,EdgeKind::Input);
    let mut im=hashbrown::HashMap::new(); im.insert(sn,SourceInfo{width:sw,height:sh,format:f}); let _=g.estimate(&im);
    match g.compile(msrc(&[sn],&[(sw,sh)],&[f],&[s])){Ok(p)=>ret(drain(p)),Err(_)=>core::ptr::null_mut()}
}
#[unsafe(no_mangle)] pub extern "C" fn op_composite(bp:*const u8,bl:u32,bw:u32,bh:u32,fp:*const u8,fl:u32,fw:u32,fh:u32,fx:u32,fy:u32,bm:u32)->*mut u8{
    let bg=unsafe{core::slice::from_raw_parts(bp,bl as usize)}; let fg=unsafe{core::slice::from_raw_parts(fp,fl as usize)}; let f=format::RGBA8_SRGB;
    let mode=match bm{1=>zenblend::BlendMode::Multiply,2=>zenblend::BlendMode::Screen,3=>zenblend::BlendMode::Overlay,_=>zenblend::BlendMode::SrcOver};
    let mut g=PipelineGraph::new(); let bn=g.add_node(NodeOp::Source); let fn_=g.add_node(NodeOp::Source);
    let cn=g.add_node(NodeOp::Composite{fg_x:fx,fg_y:fy,blend_mode:Some(mode)}); let on=g.add_node(NodeOp::Output);
    g.add_edge(bn,cn,EdgeKind::Canvas); g.add_edge(fn_,cn,EdgeKind::Input); g.add_edge(cn,on,EdgeKind::Input);
    match g.compile(msrc(&[bn,fn_],&[(bw,bh),(fw,fh)],&[f,f],&[bg,fg])){Ok(p)=>ret(drain(p)),Err(_)=>core::ptr::null_mut()}
}
#[unsafe(no_mangle)] pub extern "C" fn op_convert(sp:*const u8,sl:u32,sw:u32,sh:u32,ff:u32,tf:u32)->*mut u8{
    let s=unsafe{core::slice::from_raw_parts(sp,sl as usize)};
    let from=match ff{1=>format::RGBAF32_LINEAR,2=>format::RGB8_SRGB,_=>format::RGBA8_SRGB};
    let to=match tf{1=>format::RGBAF32_LINEAR,2=>format::RGB8_SRGB,_=>format::RGBA8_SRGB};
    let mut g=PipelineGraph::new(); let sn=g.add_node(NodeOp::Source);
    let cn=g.add_node(NodeOp::PixelTransform(Box::new(zenpipe::ops::RowConverterOp::must(from,to))));
    let on=g.add_node(NodeOp::Output); g.add_edge(sn,cn,EdgeKind::Input); g.add_edge(cn,on,EdgeKind::Input);
    match g.compile(msrc(&[sn],&[(sw,sh)],&[from],&[s])){Ok(p)=>ret(drain(p)),Err(_)=>core::ptr::null_mut()}
}
#[unsafe(no_mangle)] pub extern "C" fn op_filter(sp:*const u8,sl:u32,sw:u32,sh:u32,exposure:f32)->*mut u8{
    let s=unsafe{core::slice::from_raw_parts(sp,sl as usize)}; let f=format::RGBA8_SRGB;
    let Ok(mut pipe)=zenfilters::Pipeline::new(zenfilters::PipelineConfig::default()) else{return core::ptr::null_mut()};
    let mut e=zenfilters::filters::Exposure::default(); e.stops=exposure; pipe.push(Box::new(e));
    let mut sa=zenfilters::filters::Saturation::default(); sa.factor=1.2; pipe.push(Box::new(sa));
    let mut c=zenfilters::filters::Contrast::default(); c.amount=0.5; pipe.push(Box::new(c));
    let mut bl=zenfilters::filters::Blur::default(); bl.sigma=2.0; pipe.push(Box::new(bl));
    let mut shp=zenfilters::filters::Sharpen::default(); shp.amount=0.5; pipe.push(Box::new(shp));
    let mut g=PipelineGraph::new(); let sn=g.add_node(NodeOp::Source);
    let fn_=g.add_node(NodeOp::Filter(pipe)); let on=g.add_node(NodeOp::Output);
    g.add_edge(sn,fn_,EdgeKind::Input); g.add_edge(fn_,on,EdgeKind::Input);
    match g.compile(msrc(&[sn],&[(sw,sh)],&[f],&[s])){Ok(p)=>ret(drain(p)),Err(_)=>core::ptr::null_mut()}
}
#[unsafe(no_mangle)] pub extern "C" fn op_layout(sp:*const u8,sl:u32,sw:u32,sh:u32,dw:u32,dh:u32,ori:u8)->*mut u8{
    let s=unsafe{core::slice::from_raw_parts(sp,sl as usize)}; let f=format::RGBA8_SRGB;
    let mut g=PipelineGraph::new(); let sn=g.add_node(NodeOp::Source);
    let ln=g.add_node(NodeOp::Constrain{mode:zenresize::ConstraintMode::Within,w:dw,h:dh,orientation:if ori>1{Some(ori)}else{None},filter:Some(zenresize::Filter::Lanczos),unsharp_percent:None,gravity:None,canvas_color:None,matte_color:None,scaling_linear:None,kernel_width_scale:None,kernel_lobe_ratio:None,post_blur:None,up_filter:None,resample_when:None,sharpen_when:None});
    let on=g.add_node(NodeOp::Output); g.add_edge(sn,ln,EdgeKind::Input); g.add_edge(ln,on,EdgeKind::Input);
    match g.compile(msrc(&[sn],&[(sw,sh)],&[f],&[s])){Ok(p)=>ret(drain(p)),Err(_)=>core::ptr::null_mut()}
}
#[unsafe(no_mangle)] pub extern "C" fn op_overlay(bp:*const u8,bl:u32,bw:u32,bh:u32,op:*const u8,ol:u32,ow:u32,oh:u32,x:i32,y:i32,a:f32)->*mut u8{
    let bg=unsafe{core::slice::from_raw_parts(bp,bl as usize)}; let ov=unsafe{core::slice::from_raw_parts(op,ol as usize)}; let f=format::RGBA8_SRGB;
    let mut g=PipelineGraph::new(); let sn=g.add_node(NodeOp::Source);
    let n=g.add_node(NodeOp::Overlay{image_data:ov.to_vec(),width:ow,height:oh,format:f,x,y,opacity:a,blend_mode:None});
    let on=g.add_node(NodeOp::Output); g.add_edge(sn,n,EdgeKind::Input); g.add_edge(n,on,EdgeKind::Input);
    match g.compile(msrc(&[sn],&[(bw,bh)],&[f],&[bg])){Ok(p)=>ret(drain(p)),Err(_)=>core::ptr::null_mut()}
}
#[unsafe(no_mangle)] pub extern "C" fn op_crop_ws(sp:*const u8,sl:u32,sw:u32,sh:u32,t:u8,p:f32)->*mut u8{
    let s=unsafe{core::slice::from_raw_parts(sp,sl as usize)}; let f=format::RGBA8_SRGB;
    let mut g=PipelineGraph::new(); let sn=g.add_node(NodeOp::Source);
    let cn=g.add_node(NodeOp::CropWhitespace{threshold:t,percent_padding:p}); let on=g.add_node(NodeOp::Output);
    g.add_edge(sn,cn,EdgeKind::Input); g.add_edge(cn,on,EdgeKind::Input);
    match g.compile(msrc(&[sn],&[(sw,sh)],&[f],&[s])){Ok(p)=>ret(drain(p)),Err(_)=>core::ptr::null_mut()}
}
#[unsafe(no_mangle)] pub extern "C" fn op_alpha(sp:*const u8,sl:u32,sw:u32,sh:u32,mr:u8,mg:u8,mb:u8)->*mut u8{
    let s=unsafe{core::slice::from_raw_parts(sp,sl as usize)}; let f=format::RGBA8_SRGB;
    let mut g=PipelineGraph::new(); let sn=g.add_node(NodeOp::Source);
    let rn=g.add_node(NodeOp::RemoveAlpha{matte:[mr,mg,mb]}); let an=g.add_node(NodeOp::AddAlpha); let on=g.add_node(NodeOp::Output);
    g.add_edge(sn,rn,EdgeKind::Input); g.add_edge(rn,an,EdgeKind::Input); g.add_edge(an,on,EdgeKind::Input);
    match g.compile(msrc(&[sn],&[(sw,sh)],&[f],&[s])){Ok(p)=>ret(drain(p)),Err(_)=>core::ptr::null_mut()}
}

// === JPEG ===
#[unsafe(no_mangle)] pub extern "C" fn jpeg_encode(sp:*const u8,sl:u32,w:u32,h:u32,q:u32)->*mut u8{
    let s=unsafe{core::slice::from_raw_parts(sp,sl as usize)};
    let rgba: &[rgb::RGBA<u8>] = bytemuck::cast_slice(s);
    let cfg = zenjpeg::encoder::EncoderConfig::ycbcr(q as u8, zenjpeg::encoder::ChromaSubsampling::Quarter);
    match cfg.request().encode(rgba, w, h) { Ok(out) => ret(out), Err(_) => core::ptr::null_mut() }
}
#[unsafe(no_mangle)] pub extern "C" fn jpeg_decode(p:*const u8,l:u32,ow:*mut u32,oh:*mut u32)->*mut u8{
    generic_decode(&zenjpeg::JpegDecoderConfig::default(), unsafe{core::slice::from_raw_parts(p,l as usize)}, ow, oh)
}

// === GIF ===
#[unsafe(no_mangle)] pub extern "C" fn gif_encode(sp:*const u8,sl:u32,w:u32,h:u32)->*mut u8{
    let s=unsafe{core::slice::from_raw_parts(sp,sl as usize)};
    let pixels: Vec<zengif::Rgba> = bytemuck::cast_slice(s).to_vec();
    let frame = zengif::FrameInput::new(w as u16, h as u16, 0, pixels);
    let cfg = zengif::EncoderConfig::new();
    let limits = zengif::Limits::default();
    match zengif::encode_gif(alloc::vec![frame], w as u16, h as u16, cfg, limits, &zengif::Unstoppable) {
        Ok(out) => ret(out), Err(_) => core::ptr::null_mut()
    }
}
#[unsafe(no_mangle)] pub extern "C" fn gif_decode(p:*const u8,l:u32,ow:*mut u32,oh:*mut u32)->*mut u8{
    generic_decode(&zengif::GifDecoderConfig::default(), unsafe{core::slice::from_raw_parts(p,l as usize)}, ow, oh)
}

// === PNG ===
#[unsafe(no_mangle)] pub extern "C" fn png_encode(sp:*const u8,sl:u32,w:u32,h:u32)->*mut u8{
    let s=unsafe{core::slice::from_raw_parts(sp,sl as usize)};
    let cfg = zenpng::PngEncoderConfig::default();
    let job = cfg.dyn_job();
    let desc = zenpixels::PixelDescriptor::RGBA8_SRGB;
    let Ok(mut enc) = job.into_encoder() else { return core::ptr::null_mut() };
    let slice = zenpixels::PixelSlice::new(s, w, h, w as usize * 4, desc).unwrap();
    let _ = enc.push_rows(slice);
    match enc.finish() { Ok(out) => ret(out.data().to_vec()), Err(_) => core::ptr::null_mut() }
}
#[unsafe(no_mangle)] pub extern "C" fn png_decode(p:*const u8,l:u32,ow:*mut u32,oh:*mut u32)->*mut u8{
    generic_decode(&zenpng::PngDecoderConfig::default(), unsafe{core::slice::from_raw_parts(p,l as usize)}, ow, oh)
}

// === WebP ===
#[unsafe(no_mangle)] pub extern "C" fn webp_encode(sp:*const u8,sl:u32,w:u32,h:u32,q:u32)->*mut u8{
    let s=unsafe{core::slice::from_raw_parts(sp,sl as usize)};
    let cfg = zenwebp::zencodec::WebpEncoderConfig::lossy().with_quality(q as f32);
    let job = cfg.dyn_job();
    let desc = zenpixels::PixelDescriptor::RGBA8_SRGB;
    let Ok(mut enc) = job.into_encoder() else { return core::ptr::null_mut() };
    let slice = zenpixels::PixelSlice::new(s, w, h, w as usize * 4, desc).unwrap();
    let _ = enc.push_rows(slice);
    match enc.finish() { Ok(out) => ret(out.data().to_vec()), Err(_) => core::ptr::null_mut() }
}
#[unsafe(no_mangle)] pub extern "C" fn webp_decode(p:*const u8,l:u32,ow:*mut u32,oh:*mut u32)->*mut u8{
    generic_decode(&zenwebp::zencodec::WebpDecoderConfig::default(), unsafe{core::slice::from_raw_parts(p,l as usize)}, ow, oh)
}

// AVIF disabled — zenavif has rav1d API compat issue (lossless/base_q_idx fields)

// JXL disabled — zenjxl has API mismatch with published zencodec (parse_iso21496_jpeg renamed)

// === Memory ===
#[unsafe(no_mangle)] pub extern "C" fn free_buffer(ptr:*mut u8,len:u32){ if !ptr.is_null(){unsafe{drop(Vec::from_raw_parts(ptr,len as usize,len as usize));}} }
