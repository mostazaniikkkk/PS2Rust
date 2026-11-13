//! Variantes de coma fija 16.16 (`GLfixed`) de las funciones de GLES 1.0.
//! Convierten a `f32` y llaman a la variante de coma flotante. Los parámetros
//! que son enumerados (modo de niebla, de textura…) pasan sin escalar, como
//! manda la especificación.

#![allow(non_snake_case, clippy::missing_safety_doc)]

use crate::api::*;
use crate::consts::*;

#[inline]
fn f(x: GLfixed) -> f32 {
    x as f32 / 65536.0
}

/// Convierte `N` valores de coma fija.
unsafe fn floats<const N: usize>(p: *const GLfixed) -> [f32; N] {
    let mut out = [0.0; N];
    for (i, x) in out.iter_mut().enumerate() {
        *x = f(p.add(i).read_unaligned());
    }
    out
}

pub fn glAlphaFuncx(func: GLenum, reference: GLclampx) {
    glAlphaFunc(func, f(reference));
}

pub fn glClearColorx(red: GLclampx, green: GLclampx, blue: GLclampx, alpha: GLclampx) {
    glClearColor(f(red), f(green), f(blue), f(alpha));
}

pub fn glClearDepthx(depth: GLclampx) {
    glClearDepthf(f(depth));
}

pub fn glColor4x(red: GLfixed, green: GLfixed, blue: GLfixed, alpha: GLfixed) {
    glColor4f(f(red), f(green), f(blue), f(alpha));
}

pub fn glDepthRangex(near: GLclampx, far: GLclampx) {
    glDepthRangef(f(near), f(far));
}

pub fn glFogx(pname: GLenum, param: GLfixed) {
    glFogf(pname, if pname == GL_FOG_MODE { param as f32 } else { f(param) });
}

pub unsafe fn glFogxv(pname: GLenum, params: *const GLfixed) {
    if pname == GL_FOG_COLOR {
        glFogfv(pname, floats::<4>(params).as_ptr());
    } else {
        glFogx(pname, params.read_unaligned());
    }
}

pub fn glFrustumx(left: GLfixed, right: GLfixed, bottom: GLfixed, top: GLfixed, near: GLfixed, far: GLfixed) {
    glFrustumf(f(left), f(right), f(bottom), f(top), f(near), f(far));
}

pub fn glOrthox(left: GLfixed, right: GLfixed, bottom: GLfixed, top: GLfixed, near: GLfixed, far: GLfixed) {
    glOrthof(f(left), f(right), f(bottom), f(top), f(near), f(far));
}

pub fn glLightModelx(pname: GLenum, param: GLfixed) {
    glLightModelf(pname, f(param));
}

pub unsafe fn glLightModelxv(pname: GLenum, params: *const GLfixed) {
    if pname == GL_LIGHT_MODEL_AMBIENT {
        glLightModelfv(pname, floats::<4>(params).as_ptr());
    } else {
        glLightModelx(pname, params.read_unaligned());
    }
}

pub fn glLightx(light: GLenum, pname: GLenum, param: GLfixed) {
    glLightf(light, pname, f(param));
}

pub unsafe fn glLightxv(light: GLenum, pname: GLenum, params: *const GLfixed) {
    match pname {
        GL_AMBIENT | GL_DIFFUSE | GL_SPECULAR | GL_POSITION => glLightfv(light, pname, floats::<4>(params).as_ptr()),
        GL_SPOT_DIRECTION => glLightfv(light, pname, floats::<3>(params).as_ptr()),
        _ => glLightfv(light, pname, floats::<1>(params).as_ptr()),
    }
}

pub fn glLineWidthx(width: GLfixed) {
    glLineWidth(f(width));
}

pub unsafe fn glLoadMatrixx(m: *const GLfixed) {
    glLoadMatrixf(floats::<16>(m).as_ptr());
}

pub unsafe fn glMultMatrixx(m: *const GLfixed) {
    glMultMatrixf(floats::<16>(m).as_ptr());
}

pub fn glMaterialx(face: GLenum, pname: GLenum, param: GLfixed) {
    glMaterialf(face, pname, f(param));
}

pub unsafe fn glMaterialxv(face: GLenum, pname: GLenum, params: *const GLfixed) {
    if pname == GL_SHININESS {
        glMaterialfv(face, pname, floats::<1>(params).as_ptr());
    } else {
        glMaterialfv(face, pname, floats::<4>(params).as_ptr());
    }
}

pub fn glMultiTexCoord4x(target: GLenum, s: GLfixed, t: GLfixed, r: GLfixed, q: GLfixed) {
    glMultiTexCoord4f(target, f(s), f(t), f(r), f(q));
}

pub fn glNormal3x(nx: GLfixed, ny: GLfixed, nz: GLfixed) {
    glNormal3f(f(nx), f(ny), f(nz));
}

pub fn glPointSizex(size: GLfixed) {
    glPointSize(f(size));
}

pub fn glPolygonOffsetx(factor: GLfixed, units: GLfixed) {
    glPolygonOffset(f(factor), f(units));
}

pub fn glRotatex(angle: GLfixed, x: GLfixed, y: GLfixed, z: GLfixed) {
    glRotatef(f(angle), f(x), f(y), f(z));
}

pub fn glSampleCoveragex(value: GLclampx, invert: GLboolean) {
    glSampleCoverage(f(value), invert);
}

pub fn glScalex(x: GLfixed, y: GLfixed, z: GLfixed) {
    glScalef(f(x), f(y), f(z));
}

pub fn glTranslatex(x: GLfixed, y: GLfixed, z: GLfixed) {
    glTranslatef(f(x), f(y), f(z));
}

pub fn glTexEnvx(target: GLenum, pname: GLenum, param: GLfixed) {
    glTexEnvf(target, pname, param as f32);
}

pub unsafe fn glTexEnvxv(target: GLenum, pname: GLenum, params: *const GLfixed) {
    if pname == GL_TEXTURE_ENV_COLOR {
        glTexEnvfv(target, pname, floats::<4>(params).as_ptr());
    } else {
        glTexEnvx(target, pname, params.read_unaligned());
    }
}

pub fn glTexParameterx(target: GLenum, pname: GLenum, param: GLfixed) {
    glTexParameterf(target, pname, param as f32);
}
