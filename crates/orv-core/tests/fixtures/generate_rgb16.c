// SPDX-License-Identifier: GPL-3.0-or-later
#include <stdio.h>
#include <string.h>
#include "openjpeg.h"
int main(int argc,char **argv) {
    if(argc!=2)return 1;
    opj_cparameters_t p;opj_set_default_encoder_parameters(&p);
    p.tcp_numlayers=1;p.cp_disto_alloc=1;p.tcp_rates[0]=0;p.irreversible=0;p.numresolution=1;p.tcp_mct=1;
    opj_image_cmptparm_t c[3];memset(c,0,sizeof(c));
    for(int i=0;i<3;++i){c[i].dx=1;c[i].dy=1;c[i].w=8;c[i].h=8;c[i].prec=16;}
    opj_image_t *im=opj_image_create(3,c,OPJ_CLRSPC_SRGB);if(!im)return 2;im->x1=8;im->y1=8;
    for(int y=0;y<8;++y)for(int x=0;x<8;++x){int i=y*8+x;im->comps[0].data[i]=x*65535/7;im->comps[1].data[i]=y*65535/7;im->comps[2].data[i]=32768;}
    opj_codec_t *codec=opj_create_compress(OPJ_CODEC_J2K);if(!codec)return 3;
    if(!opj_setup_encoder(codec,&p,im))return 4;
    opj_stream_t *out=opj_stream_create_default_file_stream(argv[1],OPJ_FALSE);if(!out)return 5;
    int ok=opj_start_compress(codec,im,out)&&opj_encode(codec,out)&&opj_end_compress(codec,out);
    opj_stream_destroy(out);opj_destroy_codec(codec);opj_image_destroy(im);return ok?0:6;
}
