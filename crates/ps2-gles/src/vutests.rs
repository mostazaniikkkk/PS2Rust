//! Pruebas del microprograma de la VU1 en el PC: las mismas escenas dibujadas
//! con el camino normal (todo en Rust) y con el microprograma interpretado
//! deben dar la misma imagen.

use crate::context::{self, Context};
use crate::raster::*;
use crate::soft::SoftRasterizer;
use crate::vusoft::{VuSoftRasterizer, FAST_TRIANGLES};
use crate::*;
use std::sync::atomic::Ordering;

/// GL tiene un solo contexto actual por proceso: las pruebas que lo usan
/// van de una en una.
static GL: std::sync::Mutex<()> = std::sync::Mutex::new(());

const W: u32 = 640;
const H: u32 = 448;

/// Dibuja `scene` de las dos formas y compara las imágenes. `triangles` es el
/// número de triángulos que deben pasar por el microprograma.
fn compare(name: &str, triangles: u64, scene: &dyn Fn()) {
    let render = |raster: Box<dyn Rasterizer>| -> Vec<u8> {
        let mut context = Box::new(Context::new(raster));
        context::set_current(&mut *context);
        glClearColor(0.1, 0.2, 0.3, 1.0);
        glClear(GL_COLOR_BUFFER_BIT | GL_DEPTH_BUFFER_BIT);
        scene();
        assert_eq!(glGetError(), GL_NO_ERROR, "{name}");
        let mut pixels = vec![0u8; (W * H * 4) as usize];
        unsafe { glReadPixels(0, 0, W as i32, H as i32, GL_RGBA, GL_UNSIGNED_BYTE, pixels.as_mut_ptr().cast()) };
        context::set_current(core::ptr::null_mut());
        pixels
    };
    let reference = render(Box::new(SoftRasterizer::new(W, H, 16)));
    let before = FAST_TRIANGLES.load(Ordering::Relaxed);
    let fast = render(Box::new(VuSoftRasterizer::new(W, H, 16)));
    assert_eq!(FAST_TRIANGLES.load(Ordering::Relaxed) - before, triangles, "{name}: triángulos por el microprograma");
    let background = [26u8, 51, 77];
    let drawn = reference.chunks(4).filter(|p| p[..3] != background).count();
    assert!(drawn > 500, "{name}: la escena de referencia no dibuja nada ({drawn} píxeles)");
    // Los vértices van al GS con 4 bits de fracción y los colores con 8: en
    // los bordes puede bailar algún píxel, pero no más.
    let different = reference.chunks(4).zip(fast.chunks(4)).filter(|(a, b)| (0..4).any(|i| (a[i] as i32 - b[i] as i32).abs() > 6)).count();
    assert!(different * 200 <= drawn, "{name}: {different} píxeles distintos de {drawn} dibujados");
}

fn quad(x0: f32, y0: f32, x1: f32, y1: f32, z: f32) {
    let v = [x0, y0, z, x1, y0, z, x1, y1, z, x0, y1, z];
    unsafe {
        glEnableClientState(GL_VERTEX_ARRAY);
        glVertexPointer(3, GL_FLOAT, 0, v.as_ptr().cast());
        glDrawArrays(GL_TRIANGLE_FAN, 0, 4);
    }
}

fn perspective() {
    glMatrixMode(GL_PROJECTION);
    glLoadIdentity();
    glFrustumf(-0.7, 0.7, -0.5, 0.5, 1.0, 60.0);
    glMatrixMode(GL_MODELVIEW);
    glLoadIdentity();
}

fn checker_texture() {
    let mut name = 0;
    unsafe {
        glGenTextures(1, &mut name);
        glBindTexture(GL_TEXTURE_2D, name);
        glPixelStorei(GL_UNPACK_ALIGNMENT, 1);
        let (mut size, mut level) = (16usize, 0);
        loop {
            let texel = |i: usize| if (i % size * 2 / size + i / size * 2 / size) % 2 == 0 { [250u8, 240, 200] } else { [40, 60, 160 - level as u8 * 20] };
            let pixels: Vec<u8> = (0..size * size).flat_map(texel).collect();
            glTexImage2D(GL_TEXTURE_2D, level, GL_RGB as GLint, size as i32, size as i32, 0, GL_RGB, GL_UNSIGNED_BYTE, pixels.as_ptr().cast());
            if size == 1 {
                break;
            }
            size /= 2;
            level += 1;
        }
    }
    glTexParameterf(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_NEAREST_MIPMAP_NEAREST as f32);
    glTexParameterf(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_NEAREST as f32);
    glEnable(GL_TEXTURE_2D);
}

#[test]
fn microprogram_matches_the_reference_pipeline() {
    let _gl = GL.lock().unwrap_or_else(|e| e.into_inner());
    compare("colores interpolados y planos", 2, &|| unsafe {
        glMatrixMode(GL_PROJECTION);
        glLoadIdentity();
        glOrthof(0.0, 640.0, 0.0, 448.0, -1.0, 1.0);
        let v = [50.0f32, 40.0, 0.0, 600.0, 80.0, 0.5, 300.0, 400.0, -0.5, 20.0, 300.0, 0.0, 200.0, 310.0, 0.0, 60.0, 430.0, 0.0];
        let c: [u8; 24] = [255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 128, 255, 255, 0, 255, 0, 255, 255, 255, 255, 0, 255, 255];
        glEnableClientState(GL_VERTEX_ARRAY);
        glEnableClientState(GL_COLOR_ARRAY);
        glVertexPointer(3, GL_FLOAT, 0, v.as_ptr().cast());
        glColorPointer(4, GL_UNSIGNED_BYTE, 0, c.as_ptr().cast());
        glDrawArrays(GL_TRIANGLES, 0, 3);
        glShadeModel(GL_FLAT);
        glDrawArrays(GL_TRIANGLES, 3, 3);
    });
    compare("suelo en perspectiva recortado, con textura, mipmaps y niebla", 2, &|| unsafe {
        perspective();
        checker_texture();
        glEnable(GL_FOG);
        glFogf(GL_FOG_MODE, GL_LINEAR as f32);
        glFogf(GL_FOG_START, 2.0);
        glFogf(GL_FOG_END, 30.0);
        glFogfv(GL_FOG_COLOR, [0.1f32, 0.2, 0.3, 1.0].as_ptr());
        // Pasa por detrás de la cámara y más allá del plano lejano.
        let v = [-30.0f32, -1.0, 20.0, 30.0, -1.0, 20.0, 30.0, -1.0, -90.0, -30.0, -1.0, -90.0];
        let t = [0.0f32, 0.0, 30.0, 0.0, 30.0, 55.0, 0.0, 55.0];
        glEnableClientState(GL_VERTEX_ARRAY);
        glEnableClientState(GL_TEXTURE_COORD_ARRAY);
        glVertexPointer(3, GL_FLOAT, 0, v.as_ptr().cast());
        glTexCoordPointer(2, GL_FLOAT, 0, t.as_ptr().cast());
        glDrawArrays(GL_TRIANGLE_FAN, 0, 4);
    });
    compare("luces direccional y puntual sobre una malla indexada", 128, &|| unsafe {
        perspective();
        glTranslatef(0.0, 0.0, -5.0);
        glRotatef(25.0, 1.0, 0.3, 0.0);
        glScalef(1.5, 1.5, 1.5);
        glEnable(GL_LIGHTING);
        glEnable(GL_LIGHT0);
        glEnable(GL_LIGHT1);
        glEnable(GL_NORMALIZE);
        glEnable(GL_DEPTH_TEST);
        glEnable(GL_CULL_FACE);
        glLightfv(GL_LIGHT0, GL_POSITION, [0.3f32, 0.5, 1.0, 0.0].as_ptr());
        glLightfv(GL_LIGHT0, GL_DIFFUSE, [0.9f32, 0.6, 0.3, 1.0].as_ptr());
        glLightfv(GL_LIGHT1, GL_POSITION, [1.5f32, 1.0, 2.0, 1.0].as_ptr());
        glLightfv(GL_LIGHT1, GL_DIFFUSE, [0.2f32, 0.5, 1.0, 1.0].as_ptr());
        glLightfv(GL_LIGHT1, GL_AMBIENT, [0.1f32, 0.0, 0.0, 1.0].as_ptr());
        glLightf(GL_LIGHT1, GL_LINEAR_ATTENUATION, 0.2);
        glLightf(GL_LIGHT1, GL_QUADRATIC_ATTENUATION, 0.05);
        glMaterialfv(GL_FRONT_AND_BACK, GL_EMISSION, [0.05f32, 0.1, 0.0, 1.0].as_ptr());
        // Una rejilla ondulada de 8×8 cuadros con sus normales.
        let side = 8usize;
        let mut data: Vec<f32> = Vec::new();
        for y in 0..=side {
            for x in 0..=side {
                let (fx, fy) = (x as f32 / side as f32 * 2.0 - 1.0, y as f32 / side as f32 * 2.0 - 1.0);
                data.extend([fx, fy, (fx * 3.0).sin() * 0.25, -(fx * 3.0).cos() * 0.75, 0.0, 1.0]);
            }
        }
        let mut indices: Vec<u16> = Vec::new();
        for y in 0..side {
            for x in 0..side {
                let i = (y * (side + 1) + x) as u16;
                let j = i + side as u16 + 1;
                indices.extend([i, i + 1, j + 1, i, j + 1, j]);
            }
        }
        glEnableClientState(GL_VERTEX_ARRAY);
        glEnableClientState(GL_NORMAL_ARRAY);
        glVertexPointer(3, GL_FLOAT, 24, data.as_ptr().cast());
        glNormalPointer(GL_FLOAT, 24, data.as_ptr().add(3).cast());
        glDrawElements(GL_TRIANGLES, indices.len() as i32, GL_UNSIGNED_SHORT, indices.as_ptr().cast());
    });
    compare("color como material, reescalado y descarte de caras delanteras", 3, &|| unsafe {
        perspective();
        glTranslatef(0.0, 0.0, -4.0);
        glScalef(2.0, 2.0, 2.0);
        glEnable(GL_LIGHTING);
        glEnable(GL_LIGHT0);
        glEnable(GL_COLOR_MATERIAL);
        glEnable(GL_RESCALE_NORMAL);
        glEnable(GL_CULL_FACE);
        glCullFace(GL_FRONT);
        glFrontFace(GL_CW);
        glColor4f(0.2, 0.9, 0.5, 1.0);
        glNormal3f(0.0, 0.3, 0.9);
        quad(-0.8, -0.6, 0.8, 0.6, 0.0);
        // Éste mira hacia el otro lado: se descarta.
        let back = [0.9f32, -0.9, 0.0, 0.9, 0.9, 0.0, 1.4, 0.9, 0.0];
        glVertexPointer(3, GL_FLOAT, 0, back.as_ptr().cast());
        glDrawArrays(GL_TRIANGLES, 0, 3);
    });
    compare("triangulo gigante, viewport parcial y tiras", 5, &|| unsafe {
        glViewport(100, 50, 400, 300);
        glMatrixMode(GL_PROJECTION);
        glLoadIdentity();
        glOrthof(0.0, 400.0, 0.0, 300.0, -1.0, 1.0);
        glColor4f(1.0, 0.5, 0.0, 1.0);
        let huge = [-30000.0f32, -30000.0, 0.0, 30000.0, -30000.0, 0.0, 200.0, 250.0, 0.0];
        glEnableClientState(GL_VERTEX_ARRAY);
        glVertexPointer(3, GL_FLOAT, 0, huge.as_ptr().cast());
        glDrawArrays(GL_TRIANGLES, 0, 3);
        glColor4f(0.0, 0.5, 1.0, 0.5);
        glEnable(GL_BLEND);
        glBlendFunc(GL_SRC_ALPHA, GL_ONE_MINUS_SRC_ALPHA);
        let strip = [20.0f32, 20.0, 0.5, 120.0, 30.0, 0.5, 30.0, 120.0, 0.5, 130.0, 140.0, 0.5, 20.0, 220.0, 0.5, 140.0, 280.0, 0.5];
        glVertexPointer(3, GL_FLOAT, 0, strip.as_ptr().cast());
        glDrawArrays(GL_TRIANGLE_STRIP, 0, 6);
    });
    compare("mas triangulos de los que caben en una tanda y en un paquete", 300, &|| unsafe {
        glMatrixMode(GL_PROJECTION);
        glLoadIdentity();
        glOrthof(0.0, 640.0, 0.0, 448.0, -1.0, 1.0);
        glColor4f(0.9, 0.9, 0.2, 1.0);
        let mut v: Vec<f32> = Vec::new();
        for i in 0..300 {
            let (x, y) = ((i % 30) as f32 * 20.0 + 10.0, (i / 30) as f32 * 40.0 + 10.0);
            v.extend([x, y, 0.0, x + 16.0, y, 0.0, x, y + 30.0, 0.0]);
        }
        glEnableClientState(GL_VERTEX_ARRAY);
        glVertexPointer(3, GL_FLOAT, 0, v.as_ptr().cast());
        glDrawArrays(GL_TRIANGLES, 0, 900);
    });
}

#[test]
fn slow_path_keeps_what_the_microprogram_cannot_do() {
    let _gl = GL.lock().unwrap_or_else(|e| e.into_inner());
    // Brillo especular: la llamada entera va por el camino normal.
    let before = FAST_TRIANGLES.load(Ordering::Relaxed);
    let mut context = Box::new(Context::new(Box::new(VuSoftRasterizer::new(W, H, 16))));
    context::set_current(&mut *context);
    unsafe {
        glEnable(GL_LIGHTING);
        glEnable(GL_LIGHT0);
        glMaterialfv(GL_FRONT_AND_BACK, GL_SPECULAR, [1.0f32; 4].as_ptr());
        quad(-0.5, -0.5, 0.5, 0.5, 0.0);
        glMaterialfv(GL_FRONT_AND_BACK, GL_SPECULAR, [0.0f32, 0.0, 0.0, 1.0].as_ptr());
        glLightf(GL_LIGHT0, GL_SPOT_CUTOFF, 30.0);
        quad(-0.5, -0.5, 0.5, 0.5, 0.0);
        glDisable(GL_LIGHTING);
        glEnable(GL_FOG);
        quad(-0.5, -0.5, 0.5, 0.5, 0.0);
    }
    context::set_current(core::ptr::null_mut());
    assert_eq!(FAST_TRIANGLES.load(Ordering::Relaxed), before);
}

#[test]
fn level_of_detail_per_triangle() {
    // También cuenta triángulos en el contador común.
    let _gl = GL.lock().unwrap_or_else(|e| e.into_inner());
    // Un triángulo de `size` píxeles de lado con una textura de 64×64 entera.
    let k_for = |size: f32| -> i32 {
        let mut raster = VuSoftRasterizer::new(W, H, 16);
        let texture = TextureState {
            id: 1,
            format: TexFormat::Rgb,
            env: TexEnv::Replace,
            env_color: [0.0; 4],
            min: Filter::NearestMipNearest,
            mag: Filter::Nearest,
            wrap_s: Wrap::Repeat,
            wrap_t: Wrap::Repeat,
            levels: 7,
            width: 64,
            height: 64,
        };
        raster.set_state(&State { texture: Some(texture), ..State::DEFAULT });
        let mut mvp = [0.0f32; 16];
        (mvp[0], mvp[5], mvp[10], mvp[15]) = (2.0 / 640.0, 2.0 / 448.0, 1.0, 1.0);
        let setup = FastSetup {
            mvp,
            modelview: mvp,
            normal: [[0.0; 3]; 3],
            viewport: [0, 0, 640, 448],
            depth_range: [0.0, 1.0],
            lighting: false,
            normalize: false,
            lights: [FastLight::default(); 8],
            light_count: 0,
            base: [0.0; 3],
            scene: [1.0; 3],
            fog: None,
            cull: 0.0,
            flat: false,
        };
        let vertex = |x: f32, y: f32, s: f32, t: f32| -> FastVertex { [x, y, 0.0, s, 0.0, 0.0, 0.0, t, 1.0, 1.0, 1.0, 1.0] };
        let triangle = [vertex(0.0, 0.0, 0.0, 0.0), vertex(size, 0.0, 1.0, 0.0), vertex(0.0, size, 0.0, 1.0)];
        assert!(raster.fast_triangles(&setup, 3, &mut |out| out.copy_from_slice(&triangle)));
        raster.last_lod[0]
    };
    // K = 16 · nivel; negativo cuando la textura se amplía.
    assert_eq!(k_for(128.0), -1);
    assert_eq!(k_for(64.0), -1);
    assert_eq!(k_for(32.0), 16);
    assert_eq!(k_for(16.0), 32);
    assert_eq!(k_for(8.0), 48);
    assert_eq!(k_for(1.0), 96);
    // Entre niveles: 64/45 = 1,42 texels por píxel, nivel 0,51.
    assert!((7..=9).contains(&k_for(45.0)), "{}", k_for(45.0));
    assert!((23..=25).contains(&k_for(22.6)), "{}", k_for(22.6));
}

#[test]
fn instructions_per_triangle() {
    // También cuenta triángulos en el contador común.
    let _gl = GL.lock().unwrap_or_else(|e| e.into_inner());
    // Coste en la VU1 (una instrucción por ciclo, a 294,912 MHz) de un
    // triángulo que no hay que recortar, según lo que se le pida.
    let cost = |lights: usize, positional: bool, normalize: bool| -> u64 {
        let mut raster = VuSoftRasterizer::new(W, H, 16);
        raster.set_state(&State::DEFAULT);
        let mut mvp = [0.0f32; 16];
        (mvp[0], mvp[5], mvp[10], mvp[15]) = (2.0 / 640.0, 2.0 / 448.0, 1.0, 1.0);
        let light = FastLight { vector: [0.0, 0.0, 1.0], positional, ambient: [0.1; 3], diffuse: [0.8; 3], attenuation: [1.0, 0.0, 0.0] };
        let setup = FastSetup {
            mvp,
            modelview: mvp,
            normal: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            viewport: [0, 0, 640, 448],
            depth_range: [0.0, 1.0],
            lighting: lights > 0,
            normalize,
            lights: [light; 8],
            light_count: lights,
            base: [0.0; 3],
            scene: [0.2; 3],
            fog: None,
            cull: 1.0,
            flat: false,
        };
        let triangle: [FastVertex; 3] = [[10.0, 10.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 1.0, 1.0, 1.0, 1.0], [90.0, 10.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 1.0, 1.0, 1.0, 1.0], [10.0, 90.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 1.0, 1.0, 1.0, 1.0]];
        let triangles = 400;
        assert!(raster.fast_triangles(&setup, triangles * 3, &mut |out| out.chunks_mut(3).for_each(|t| t.copy_from_slice(&triangle))));
        raster.executed() / triangles as u64
    };
    let (unlit, one, normalized, point, eight) = (cost(0, false, false), cost(1, false, false), cost(1, false, true), cost(1, true, true), cost(8, false, true));
    println!("instrucciones por triángulo: sin luces {unlit}, 1 direccional {one}, + normalizar {normalized}, 1 puntual {point}, 8 direccionales {eight}");
    // Si esto crece, que sea a propósito.
    assert!(unlit <= 220 && one <= 300 && normalized <= 330 && point <= 420 && eight <= 600, "el microprograma se ha encarecido");
}
