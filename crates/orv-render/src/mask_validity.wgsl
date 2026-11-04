// SPDX-License-Identifier: GPL-3.0-or-later
fn source_eligible(cell:vec3i)->bool {
    return p.control.y<0.5 || textureLoad(validity,cell,0).r!=0u;
}
