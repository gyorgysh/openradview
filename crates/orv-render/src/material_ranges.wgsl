// SPDX-License-Identifier: GPL-3.0-or-later
fn transparent_advance(stencil_lower:vec3u,point:vec3f,origin:vec3f,near:f32,step:f32,index:u32,count:u32)->u32 {
    let dimensions=source_dimensions();
    // The final scalar node belongs to the last interpolation cell. The stored
    // positive halo covers all eight vertices read by stencil_scalar.
    let lower=min(stencil_lower,dimensions-2u);
    let tile=lower/range_grid.x;
    if textureLoad(transparent_tiles,vec3i(tile),0).r==0u {return 0u;}
    // Fall back to one certified zero sample when the affine expression cannot
    // meet the bounded-error contract. No rounded boundary division is trusted.
    if count>4096u || abs(near)>1e6 || !(step>=1e-6 && step<=1e6) ||
        any(abs(origin)>vec3f(4096.)) || any(abs(p.direction.xyz)>vec3f(1e6)) ||
        any(abs(p.direction.xyz*near)>vec3f(4096.)) ||
        any(abs(p.direction.xyz*(f32(count)*step))>vec3f(4096.)) ||
        any(dimensions>vec3u(1048576u)) {return 1u;}
    // Under those limits every allowed affine evaluation is within .125 voxel
    // of its exact line. Endpoints one voxel inside the integer tile certify
    // all intervening original sample indices, including rounding and FTZ.
    let low=vec3f(tile*range_grid.x)+1.;
    let high=vec3f(min((tile+1u)*range_grid.x,dimensions-1u))-1.;
    if any(point<low) || any(point>high) {return 1u;}
    var distance=f32(count-index-1u);
    let delta=p.direction.xyz*step;
    for(var axis=0u;axis<3u;axis++) {
        // Division proposes an index only. Every accepted endpoint is checked
        // with the original affine expression and the same interior margin.
        if abs(delta[axis])>=1e-20 {
            let boundary=select(low[axis],high[axis],delta[axis]>0.);
            distance=min(distance,max(0.,floor((boundary-point[axis])/delta[axis])));
        }
    }
    var advance=u32(distance);
    for(var attempt=0u;attempt<3u;attempt++) {
        let end=material_point(origin,near,step,index+advance);
        if all(end>=low) && all(end<=high) {return advance+1u;}
        advance/=2u;
    }
    return 1u;
}
