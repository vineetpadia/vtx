#define WIN32_LEAN_AND_MEAN
#define NOMINMAX
#include <windows.h>
#include <vector>
#include <string>
#include <algorithm>
#include <iostream>
#include "saved_preview.h"

static void Word(std::vector<BYTE>& bytes,size_t at,uint32_t value){memcpy(bytes.data()+at,&value,4);}
static std::vector<BYTE> Block(const std::vector<BYTE>& png,int format){
    std::vector<BYTE> payload=png;
    if(format){
        auto compressor=libdeflate_alloc_compressor(1);
        std::vector<BYTE> compressed(1024);
        size_t n=format==1?libdeflate_deflate_compress(compressor,png.data(),png.size(),compressed.data(),compressed.size()):libdeflate_zlib_compress(compressor,png.data(),png.size(),compressed.data(),compressed.size());
        libdeflate_free_compressor(compressor);compressed.resize(n);payload=std::move(compressed);
    }
    std::vector<BYTE> bytes(26+10+payload.size());
    const BYTE marker[]={0x14,0,6,0,8,0};memcpy(bytes.data(),marker,6);
    Word(bytes,10,libdeflate_crc32(0,png.data(),png.size()));Word(bytes,14,uint32_t(payload.size()));Word(bytes,18,uint32_t(png.size()));Word(bytes,22,10);
    const char* name="PreviewPNG";for(size_t i=0;i<10;++i)bytes[26+i]=BYTE((name[i]<<4)|(BYTE(name[i])>>4));
    memcpy(bytes.data()+36,payload.data(),payload.size());return bytes;
}
int main(){
    std::vector<BYTE> png={137,80,78,71,13,10,26,10,0,0,0,13,'I','H','D','R',0,0,1,0,0,0,1,0};
    std::vector<BYTE> output;
    for(int format:{0,1,2}){
        auto block=Block(png,format);
        if(!ExtractSavedPng(block,output)||output!=png)return 1;
        block[10]^=1;if(ExtractSavedPng(block,output))return 2;
    }
    auto block=Block(png,0);
    for(size_t length=0;length<block.size();++length){std::vector<BYTE> truncated(block.begin(),block.begin()+length);if(ExtractSavedPng(truncated,output))return 3;}
    Word(block,18,0xffffffff);if(ExtractSavedPng(block,output))return 4;
    block=Block(png,0);Word(block,14,0xffffffff);if(ExtractSavedPng(block,output))return 5;
    png[16]=1;block=Block(png,0);if(ExtractSavedPng(block,output))return 6;
    png[16]=0;png[12]='X';block=Block(png,0);if(ExtractSavedPng(block,output))return 7;
    std::cout<<"PASS: raw/deflate/zlib, CRC rejection, every truncation, length limits, PNG bounds and IHDR\n";
    return 0;
}
