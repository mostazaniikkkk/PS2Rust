//! Pruebas del microprograma de la VU1 en el PC: las mismas escenas dibujadas
//! con el camino normal (todo en Rust) y con el microprograma interpretado
//! deben dar la misma imagen.

use crate::context::{self, Context};
use crate::raster::*;
use crate::soft::SoftRasterizer;
use crate::vusoft::{VuSoftRasterizer, FAST_TRIANGLES, FILLED_VERTICES};

type FastVertex = [f32; 12];
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
            projection: mvp,
            skin: 0,
            palette: &[],
            const_color: None,
        };
        let vertex = |x: f32, y: f32, s: f32, t: f32| -> FastVertex { [x, y, 0.0, s, 0.0, 0.0, 0.0, t, 1.0, 1.0, 1.0, 1.0] };
        let triangle = [vertex(0.0, 0.0, 0.0, 0.0), vertex(size, 0.0, 1.0, 0.0), vertex(0.0, size, 0.0, 1.0)];
        assert!(raster.fast_triangles(&setup, 3, &mut |out| out.copy_from_slice(triangle.as_flattened())));
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
            projection: mvp,
            skin: 0,
            palette: &[],
            const_color: None,
        };
        let triangle: [FastVertex; 3] = [[10.0, 10.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 1.0, 1.0, 1.0, 1.0], [90.0, 10.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 1.0, 1.0, 1.0, 1.0], [10.0, 90.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 1.0, 1.0, 1.0, 1.0]];
        let triangles = 400;
        assert!(raster.fast_triangles(&setup, triangles * 3, &mut |out| out.chunks_mut(36).for_each(|t| t.copy_from_slice(triangle.as_flattened()))));
        raster.executed() / triangles as u64
    };
    let (unlit, one, normalized, point, eight) = (cost(0, false, false), cost(1, false, false), cost(1, false, true), cost(1, true, true), cost(8, false, true));
    println!("instrucciones por triángulo: sin luces {unlit}, 1 direccional {one}, + normalizar {normalized}, 1 puntual {point}, 8 direccionales {eight}");
    // Si esto crece, que sea a propósito.
    assert!(unlit <= 220 && one <= 300 && normalized <= 330 && point <= 420 && eight <= 600, "el microprograma se ha encarecido");
}

// ---------------------------------------------------------------- búferes y paleta de matrices

/// Una cinta a lo largo de x, de `segments` tramos, con tres huesos: el
/// primero manda en un extremo, el segundo en el centro y el tercero en el
/// otro extremo.
struct Ribbon {
    positions: Vec<f32>,
    normals: Vec<f32>,
    weights: Vec<f32>,
    bones: Vec<u8>,
    colors: Vec<u8>,
    indices: Vec<u16>,
}

fn ribbon(segments: usize) -> Ribbon {
    let mut r = Ribbon { positions: Vec::new(), normals: Vec::new(), weights: Vec::new(), bones: Vec::new(), colors: Vec::new(), indices: Vec::new() };
    for i in 0..=segments {
        let t = i as f32 / segments as f32;
        for y in [-0.7f32, 0.7] {
            r.positions.extend([t * 6.0 - 3.0, y, 0.0]);
            r.normals.extend([0.0, 0.0, 1.0]);
            r.weights.extend(if t < 0.5 { [1.0 - 2.0 * t, 2.0 * t, 0.0] } else { [0.0, 2.0 - 2.0 * t, 2.0 * t - 1.0] });
            r.bones.extend([0u8, 1, 2]);
            r.colors.extend([(255.0 * t) as u8, 200, (255.0 * (1.0 - t)) as u8, 255]);
        }
    }
    for i in 0..segments as u16 {
        let a = i * 2;
        r.indices.extend([a, a + 2, a + 1, a + 1, a + 2, a + 3]);
    }
    r
}

/// Las tres matrices de la cinta: los extremos giran `bend` grados.
fn bones(bend: f32) {
    glMatrixMode(GL_MATRIX_PALETTE_OES);
    for (i, angle) in [-bend, 0.0, bend].into_iter().enumerate() {
        glCurrentPaletteMatrixOES(i as GLuint);
        glLoadIdentity();
        glTranslatef(0.0, 0.0, -9.0);
        glRotatef(angle, 0.0, 1.0, 0.0);
        if i == 2 {
            // Escala no uniforme: las normales necesitan la inversa de verdad.
            glScalef(1.0, 1.4, 1.0);
        }
    }
    glMatrixMode(GL_MODELVIEW);
}

fn lights() {
    glEnable(GL_LIGHTING);
    glEnable(GL_LIGHT0);
    glEnable(GL_LIGHT1);
    unsafe {
        glLightfv(GL_LIGHT1, GL_POSITION, [2.0f32, 1.0, -6.0, 1.0].as_ptr());
        glLightfv(GL_LIGHT1, GL_DIFFUSE, [0.9f32, 0.3, 0.1, 1.0].as_ptr());
    }
    glLightf(GL_LIGHT1, GL_LINEAR_ATTENUATION, 0.2);
}

/// Dibuja la cinta desde arrays del programa, con `units` matrices por vértice.
fn draw_ribbon(r: &Ribbon, units: i32) {
    unsafe {
        glEnable(GL_MATRIX_PALETTE_OES);
        glEnableClientState(GL_VERTEX_ARRAY);
        glEnableClientState(GL_NORMAL_ARRAY);
        glEnableClientState(GL_MATRIX_INDEX_ARRAY_OES);
        glEnableClientState(GL_WEIGHT_ARRAY_OES);
        glVertexPointer(3, GL_FLOAT, 0, r.positions.as_ptr().cast());
        glNormalPointer(GL_FLOAT, 0, r.normals.as_ptr().cast());
        glMatrixIndexPointerOES(units, GL_UNSIGNED_BYTE, 3, r.bones.as_ptr().cast());
        glWeightPointerOES(units, GL_FLOAT, 12, r.weights.as_ptr().cast());
        glDrawElements(GL_TRIANGLES, r.indices.len() as i32, GL_UNSIGNED_SHORT, r.indices.as_ptr().cast());
    }
}

#[test]
fn skinning_lit() {
    let _gl = GL.lock().unwrap_or_else(|e| e.into_inner());
    let r = ribbon(16);
    compare("paleta de matrices con luces", 32, &|| {
        perspective();
        glEnable(GL_DEPTH_TEST);
        glEnable(GL_CULL_FACE);
        lights();
        bones(35.0);
        draw_ribbon(&r, 3);
    });
    compare("paleta de matrices con luces, normalizando", 32, &|| {
        perspective();
        lights();
        glEnable(GL_NORMALIZE);
        bones(-50.0);
        draw_ribbon(&r, 3);
    });
}

#[test]
fn skinning_units() {
    let _gl = GL.lock().unwrap_or_else(|e| e.into_inner());
    let r = ribbon(16);
    // Con una o dos matrices por vértice sólo cuentan los primeros pesos (no
    // suman 1: la cinta se encoge hacia el ojo, igual en los dos caminos).
    for units in [1, 2] {
        compare("paleta con menos matrices por vértice", 32, &|| {
            perspective();
            glMatrixMode(GL_PROJECTION);
            glLoadIdentity();
            glOrthof(-4.0, 4.0, -3.0, 3.0, -20.0, 20.0);
            glMatrixMode(GL_MODELVIEW);
            bones(30.0);
            glColor4f(0.9, 0.8, 0.2, 1.0);
            draw_ribbon(&r, units);
        });
    }
    // Colores por vértice y niebla, sin luces; recorte contra el plano cercano.
    compare("paleta con colores, niebla y recorte", 32, &|| {
        perspective();
        glEnable(GL_FOG);
        glFogf(GL_FOG_MODE, GL_LINEAR as f32);
        glFogf(GL_FOG_START, 2.0);
        glFogf(GL_FOG_END, 12.0);
        unsafe { glFogfv(GL_FOG_COLOR, [0.1f32, 0.2, 0.3, 1.0].as_ptr()) };
        bones(80.0);
        unsafe {
            glEnableClientState(GL_COLOR_ARRAY);
            glColorPointer(4, GL_UNSIGNED_BYTE, 0, r.colors.as_ptr().cast());
        }
        draw_ribbon(&r, 3);
    });
}

/// Sube la cinta a objetos de búfer y deja los arrays apuntando a ellos.
fn ribbon_buffers(r: &Ribbon, usage: GLenum) -> [GLuint; 5] {
    let mut names = [0; 5];
    unsafe {
        glGenBuffers(5, names.as_mut_ptr());
        let upload = |target: GLenum, name: GLuint, bytes: usize, data: *const GLvoid| {
            glBindBuffer(target, name);
            glBufferData(target, bytes as GLsizeiptr, data, usage);
        };
        upload(GL_ARRAY_BUFFER, names[0], r.positions.len() * 4, r.positions.as_ptr().cast());
        glVertexPointer(3, GL_FLOAT, 0, core::ptr::null());
        upload(GL_ARRAY_BUFFER, names[1], r.normals.len() * 4, r.normals.as_ptr().cast());
        glNormalPointer(GL_FLOAT, 0, core::ptr::null());
        upload(GL_ARRAY_BUFFER, names[2], r.weights.len() * 4, r.weights.as_ptr().cast());
        glWeightPointerOES(3, GL_FLOAT, 0, core::ptr::null());
        upload(GL_ARRAY_BUFFER, names[3], r.bones.len(), r.bones.as_ptr().cast());
        glMatrixIndexPointerOES(3, GL_UNSIGNED_BYTE, 0, core::ptr::null());
        upload(GL_ELEMENT_ARRAY_BUFFER, names[4], r.indices.len() * 2, r.indices.as_ptr().cast());
        glEnableClientState(GL_VERTEX_ARRAY);
        glEnableClientState(GL_NORMAL_ARRAY);
    }
    names
}

/// Vértices que el núcleo de GL entrega al rasterizador de la VU mientras
/// se ejecuta `scene` (comparando además con la referencia).
fn filled(name: &str, triangles: u64, scene: &dyn Fn()) -> u64 {
    let before = FILLED_VERTICES.load(Ordering::Relaxed);
    compare(name, triangles, scene);
    FILLED_VERTICES.load(Ordering::Relaxed) - before
}

#[test]
fn retained_buffers() {
    let _gl = GL.lock().unwrap_or_else(|e| e.into_inner());
    let r = ribbon(16);
    let draw = |x: f32, y: f32| unsafe {
        glLoadIdentity();
        glTranslatef(x, y, -9.0);
        glDrawElements(GL_TRIANGLES, 96, GL_UNSIGNED_SHORT, core::ptr::null());
    };
    // Estática: tres dibujos con otra matriz, los vértices se leen una vez.
    let vertices = filled("búfer estático, tres veces", 96, &|| {
        perspective();
        lights();
        ribbon_buffers(&r, GL_STATIC_DRAW);
        draw(0.0, -2.0);
        draw(0.0, 0.0);
        draw(0.5, 2.0);
        assert_eq!(glGetError(), GL_NO_ERROR);
    });
    assert_eq!(vertices, 96);
    // Dinámica: no se retiene.
    let vertices = filled("búfer dinámico", 64, &|| {
        perspective();
        lights();
        ribbon_buffers(&r, GL_DYNAMIC_DRAW);
        draw(0.0, -1.0);
        draw(0.0, 1.0);
    });
    assert_eq!(vertices, 192);
    // Cambiar el contenido obliga a leerla de nuevo, y se ve el cambio.
    let vertices = filled("búfer estático que cambia", 96, &|| {
        perspective();
        lights();
        let names = ribbon_buffers(&r, GL_STATIC_DRAW);
        draw(0.0, -2.0);
        draw(0.0, 0.0);
        let moved: Vec<f32> = r.positions.iter().enumerate().map(|(i, v)| if i % 3 == 1 { v * 0.5 } else { *v }).collect();
        unsafe {
            glBindBuffer(GL_ARRAY_BUFFER, names[0]);
            glBufferSubData(GL_ARRAY_BUFFER, 0, (moved.len() * 4) as GLsizeiptr, moved.as_ptr().cast());
        }
        draw(0.0, 2.0);
    });
    assert_eq!(vertices, 192);
    // Un trozo de la misma malla es otra geometría; sin sitio no se retiene.
    let vertices = filled("trozos y límite de memoria", 96, &|| {
        perspective();
        lights();
        ribbon_buffers(&r, GL_STATIC_DRAW);
        unsafe {
            glLoadIdentity();
            glTranslatef(0.0, -2.0, -9.0);
            glDrawElements(GL_TRIANGLES, 48, GL_UNSIGNED_SHORT, 48 as *const GLvoid);
            glDrawElements(GL_TRIANGLES, 48, GL_UNSIGNED_SHORT, 48 as *const GLvoid);
        }
        set_retained_budget(0);
        assert_eq!(retained_bytes(), 0);
        draw(0.0, 0.0);
        draw(0.0, 2.0);
    });
    assert_eq!(vertices, 48 + 96 + 96);
    // Entradas que no vienen del búfer (aquí la normal actual): si cambian a
    // cada dibujo se deja de retener, y siempre se dibuja con la buena.
    let vertices = filled("normal actual que cambia", 32 * 6, &|| {
        perspective();
        lights();
        ribbon_buffers(&r, GL_STATIC_DRAW);
        glDisableClientState(GL_NORMAL_ARRAY);
        for i in 0..6 {
            glNormal3f(0.0, i as f32 * 0.15, 1.0 - i as f32 * 0.1);
            draw(0.0, i as f32 - 2.5);
        }
    });
    assert_eq!(vertices, 96 * 6);
}

#[test]
fn retained_skinning() {
    let _gl = GL.lock().unwrap_or_else(|e| e.into_inner());
    let r = ribbon(16);
    // La malla animada: se retiene y, de un dibujo a otro, sólo cambia la paleta.
    let vertices = filled("paleta de matrices sobre búferes", 96, &|| {
        perspective();
        lights();
        glEnable(GL_NORMALIZE);
        ribbon_buffers(&r, GL_STATIC_DRAW);
        glEnable(GL_MATRIX_PALETTE_OES);
        glEnableClientState(GL_MATRIX_INDEX_ARRAY_OES);
        glEnableClientState(GL_WEIGHT_ARRAY_OES);
        for (i, bend) in [-60.0f32, 10.0, 70.0].into_iter().enumerate() {
            bones(bend);
            glMatrixMode(GL_MATRIX_PALETTE_OES);
            for matrix in 0..3 {
                glCurrentPaletteMatrixOES(matrix);
                // Desplaza cada copia en y, antes de la matriz del hueso.
                let mut shifted = crate::math::IDENTITY;
                shifted[13] = i as f32 * 2.0 - 2.0;
                unsafe { glMultMatrixf(shifted.as_ptr()) };
            }
            glMatrixMode(GL_MODELVIEW);
            unsafe { glDrawElements(GL_TRIANGLES, 96, GL_UNSIGNED_SHORT, core::ptr::null()) };
        }
    });
    assert_eq!(vertices, 96);
}

#[test]
fn skinning_cost() {
    let _gl = GL.lock().unwrap_or_else(|e| e.into_inner());
    let cost = |units: usize, lit: bool| -> u64 {
        let mut raster = VuSoftRasterizer::new(W, H, 16);
        raster.set_state(&State::DEFAULT);
        let mut projection = [0.0f32; 16];
        (projection[0], projection[5], projection[10], projection[15]) = (2.0 / 640.0, 2.0 / 448.0, 1.0, 1.0);
        let palette = [crate::math::IDENTITY; FAST_PALETTE_SIZE];
        let light = FastLight { vector: [0.0, 0.0, 1.0], positional: false, ambient: [0.1; 3], diffuse: [0.8; 3], attenuation: [1.0, 0.0, 0.0] };
        let setup = FastSetup {
            mvp: projection,
            modelview: crate::math::IDENTITY,
            normal: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            viewport: [0, 0, 640, 448],
            depth_range: [0.0, 1.0],
            lighting: lit,
            normalize: lit,
            lights: [light; 8],
            light_count: lit as usize,
            base: [0.0; 3],
            scene: [0.2; 3],
            fog: None,
            cull: 1.0,
            flat: false,
            projection,
            skin: units,
            palette: &palette,
            const_color: Some([1.0; 4]),
        };
        let address = f32::from_bits(FAST_PALETTE_BASE);
        let vertex = |x: f32, y: f32| [x, y, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, address, address, address, address];
        let triangle = [vertex(10.0, 10.0), vertex(90.0, 10.0), vertex(10.0, 90.0)];
        let triangles = 300;
        assert!(raster.fast_triangles(&setup, triangles * 3, &mut |out| out.chunks_mut(60).for_each(|t| t.copy_from_slice(triangle.as_flattened()))));
        raster.executed() / triangles as u64
    };
    let (one, four, lit) = (cost(1, false), cost(4, false), cost(4, true));
    println!("instrucciones por triángulo con paleta: 1 matriz {one}, 4 matrices {four}, 4 con una luz {lit}");
    assert!(one <= 250 && four <= 340 && lit <= 460, "el microprograma se ha encarecido");
}
