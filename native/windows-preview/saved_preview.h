#pragma once
#include <libdeflate.h>
#include <cstring>

// Format layout: vendor/sldprt/NOTICE.md. Never decode geometry here.
static uint32_t LittleWord(const BYTE* p){uint32_t n;memcpy(&n,p,4);return n;}
static bool ExtractSavedPng(const std::vector<BYTE>& bytes,std::vector<BYTE>& png){
    const BYTE marker[]={0x14,0,6,0,8,0};
    const BYTE name[]={0x05,0x27,0x56,0x67,0x96,0x56,0x77,0x05,0xe4,0x74}; // PreviewPNG, nibble swapped
    const size_t limit=16*1024*1024;
    for(size_t at=26;at+sizeof(name)<=bytes.size();++at){
        if(bytes[at]!=name[0]||memcmp(bytes.data()+at,name,sizeof(name)))continue;
        const BYTE* header=bytes.data()+at-26;
        if(memcmp(header,marker,sizeof(marker))||LittleWord(header+22)!=sizeof(name))continue;
        size_t compressed=LittleWord(header+14),expanded=LittleWord(header+18),payload=at+sizeof(name);
        if(!expanded||expanded>limit||!compressed||compressed>limit||compressed>bytes.size()-payload)continue;
        std::vector<BYTE> decoded(expanded);
        auto decoder=libdeflate_alloc_decompressor();if(!decoder)return false;
        size_t actual=0;
        bool valid=libdeflate_deflate_decompress(decoder,bytes.data()+payload,compressed,decoded.data(),expanded,&actual)==LIBDEFLATE_SUCCESS&&actual==expanded;
        if(!valid)valid=libdeflate_zlib_decompress(decoder,bytes.data()+payload,compressed,decoded.data(),expanded,&actual)==LIBDEFLATE_SUCCESS&&actual==expanded;
        libdeflate_free_decompressor(decoder);
        if(!valid&&compressed==expanded){memcpy(decoded.data(),bytes.data()+payload,expanded);valid=true;}
        if(!valid||libdeflate_crc32(0,decoded.data(),expanded)!=LittleWord(header+10))continue;
        const BYTE signature[]={137,80,78,71,13,10,26,10};
        if(decoded.size()<24||memcmp(decoded.data(),signature,8)||memcmp(decoded.data()+12,"IHDR",4))continue;
        auto dimension=[&](size_t p){return (uint32_t(decoded[p])<<24)|(uint32_t(decoded[p+1])<<16)|(uint32_t(decoded[p+2])<<8)|decoded[p+3];};
        if(!dimension(16)||!dimension(20)||dimension(16)>4096||dimension(20)>4096)continue;
        png=std::move(decoded);return true;
    }
    return false;
}
static bool ReadSavedPng(const std::wstring& path,std::vector<BYTE>& png){
    HANDLE source=CreateFileW(path.c_str(),GENERIC_READ,FILE_SHARE_READ|FILE_SHARE_WRITE|FILE_SHARE_DELETE,nullptr,OPEN_EXISTING,FILE_FLAG_RANDOM_ACCESS,nullptr);
    if(source==INVALID_HANDLE_VALUE)return false;
    LARGE_INTEGER length{};bool ready=false;
    if(GetFileSizeEx(source,&length)&&length.QuadPart>0){
        for(DWORD budget:{256u*1024u,4u*1024u*1024u}){
            DWORD count=DWORD(std::min<LONGLONG>(length.QuadPart,budget));
            LARGE_INTEGER offset{};offset.QuadPart=length.QuadPart-count;
            std::vector<BYTE> bytes(count);DWORD read=0;
            if(SetFilePointerEx(source,offset,nullptr,FILE_BEGIN)&&ReadFile(source,bytes.data(),count,&read,nullptr)&&read==count&&ExtractSavedPng(bytes,png)){ready=true;break;}
            if(length.QuadPart<=budget)break;
        }
    }
    CloseHandle(source);return ready;
}
