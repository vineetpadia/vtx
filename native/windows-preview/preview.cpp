// A read-only, per-user Explorer preview handler. Geometry decoding stays in
// VTX.exe, outside Explorer/prevhost. A saved image appears before the live view.
#define WIN32_LEAN_AND_MEAN
#define NOMINMAX
#include <windows.h>
#include <shobjidl.h>
#include <shlobj.h>
#include <wincodec.h>
#include <wrl/client.h>
#include <string>
#include <vector>
#include <algorithm>
#include <cwctype>
#include <new>
#include "saved_preview.h"
using Microsoft::WRL::ComPtr;

// Separate class allows a live upgrade without restarting Explorer/prevhost.
// {75660313-4B6C-45D7-96D9-39608F52F040}
static const CLSID CLSID_VtxPreview = {0x75660313,0x4b6c,0x45d7,{0x96,0xd9,0x39,0x60,0x8f,0x52,0xf0,0x40}};
static HMODULE module;
static LONG objects = 0;
static const wchar_t* windowClass = L"VTXExplorerPreview";

static std::wstring Quote(const std::wstring& value) {
    std::wstring result=L"\"";
    size_t slashes=0;
    for (wchar_t c:value) {
        if(c==L'\\'){++slashes;continue;}
        result.append(c==L'"'?slashes*2+1:slashes,L'\\');slashes=0;
        result+=c;
    }
    result.append(slashes*2,L'\\');return result+L"\"";
}
static std::wstring BaseName(const std::wstring& path){return path.substr(path.find_last_of(L"\\/")+1);}
static std::wstring Extension(const std::wstring& path){auto at=path.find_last_of(L'.');auto s=at==std::wstring::npos?L"":path.substr(at);std::transform(s.begin(),s.end(),s.begin(),towlower);return s;}
static bool IsFile(const std::wstring& path){auto a=GetFileAttributesW(path.c_str());return a!=INVALID_FILE_ATTRIBUTES && !(a&FILE_ATTRIBUTE_DIRECTORY);}

class Preview final : public IPreviewHandler, public IInitializeWithFile,
    public IObjectWithSite, public IOleWindow, public IPreviewHandlerVisuals {
    LONG references=1;
    HWND parent=nullptr,window=nullptr,live=nullptr;
    RECT rect{};
    ComPtr<IUnknown> site;
    ComPtr<IWICImagingFactory> imageFactory;
    std::wstring file,cache,temp,executable,status=L"Loading preview...";
    HBITMAP bitmap=nullptr,scaledBitmap=nullptr;
    UINT imageWidth=0,imageHeight=0;
    UINT scaledWidth=0,scaledHeight=0;
    HFONT font=nullptr;
    COLORREF background=RGB(255,255,255),text=RGB(40,48,60);
    HANDLE worker=nullptr,job=nullptr;
    ULONGLONG started=0;
    bool savedView=false,partial=false;

    void Cancel(){
        if(window){KillTimer(window,1);KillTimer(window,2);KillTimer(window,3);}
        if(worker){if(WaitForSingleObject(worker,0)==WAIT_TIMEOUT){TerminateProcess(worker,ERROR_CANCELLED);WaitForSingleObject(worker,2000);}CloseHandle(worker);worker=nullptr;}
        if(job){CloseHandle(job);job=nullptr;}live=nullptr;savedView=partial=false;
        if(!temp.empty())DeleteFileW(temp.c_str());
    }
    bool Factory(){return imageFactory||SUCCEEDED(CoCreateInstance(CLSID_WICImagingFactory,nullptr,CLSCTX_INPROC_SERVER,IID_PPV_ARGS(&imageFactory)));}
    bool Decode(IWICBitmapDecoder* decoder){
        ComPtr<IWICBitmapFrameDecode> frame;
        if(FAILED(decoder->GetFrame(0,&frame)))return false;
        UINT w=0,h=0;
        if(FAILED(frame->GetSize(&w,&h))||!w||!h||w>4096||h>4096)return false;
        ComPtr<IWICFormatConverter> converter;
        if(FAILED(imageFactory->CreateFormatConverter(&converter))||FAILED(converter->Initialize(frame.Get(),GUID_WICPixelFormat32bppPBGRA,WICBitmapDitherTypeNone,nullptr,0,WICBitmapPaletteTypeCustom)))return false;
        BITMAPINFO info{};info.bmiHeader.biSize=sizeof(BITMAPINFOHEADER);info.bmiHeader.biWidth=w;info.bmiHeader.biHeight=-LONG(h);info.bmiHeader.biPlanes=1;info.bmiHeader.biBitCount=32;info.bmiHeader.biCompression=BI_RGB;
        void* pixels=nullptr;
        HBITMAP next=CreateDIBSection(nullptr,&info,DIB_RGB_COLORS,&pixels,nullptr,0);
        if(!next)return false;
        if(FAILED(converter->CopyPixels(nullptr,w*4,w*h*4,static_cast<BYTE*>(pixels)))){DeleteObject(next);return false;}
        if(bitmap)DeleteObject(bitmap);if(scaledBitmap)DeleteObject(scaledBitmap);scaledBitmap=nullptr;bitmap=next;imageWidth=w;imageHeight=h;return true;
    }
    bool LoadImage(const std::wstring& path){
        if(!Factory())return false;
        ComPtr<IWICBitmapDecoder> decoder;
        return SUCCEEDED(imageFactory->CreateDecoderFromFilename(path.c_str(),nullptr,GENERIC_READ,WICDecodeMetadataCacheOnLoad,&decoder))&&Decode(decoder.Get());
    }
    bool LoadSavedImage(){
        std::vector<BYTE> png;if(!ReadSavedPng(file,png)||!Factory())return false;
        ComPtr<IWICStream> stream;ComPtr<IWICBitmapDecoder> decoder;
        return SUCCEEDED(imageFactory->CreateStream(&stream))&&SUCCEEDED(stream->InitializeFromMemory(png.data(),DWORD(png.size())))&&SUCCEEDED(imageFactory->CreateDecoderFromStream(stream.Get(),nullptr,WICDecodeMetadataCacheOnLoad,&decoder))&&Decode(decoder.Get());
    }
    bool PrepareCache(){
        PWSTR location=nullptr;
        if(FAILED(SHGetKnownFolderPath(FOLDERID_LocalAppDataLow,KF_FLAG_CREATE,nullptr,&location)))return false;
        std::wstring folder=std::wstring(location)+L"\\VTX\\Previews";CoTaskMemFree(location);
        if(SHCreateDirectoryExW(nullptr,folder.c_str(),nullptr)!=ERROR_SUCCESS&&!IsFile(folder)){
            auto a=GetFileAttributesW(folder.c_str());if(a==INVALID_FILE_ATTRIBUTES||!(a&FILE_ATTRIBUTE_DIRECTORY))return false;
        }
        ULONGLONG hash=14695981039346656037ull;
        auto add=[&](const void* p,size_t n){auto b=static_cast<const BYTE*>(p);for(size_t i=0;i<n;++i){hash^=b[i];hash*=1099511628211ull;}};
        std::wstring lower=file;std::transform(lower.begin(),lower.end(),lower.begin(),towlower);add(lower.data(),lower.size()*sizeof(wchar_t));
        WIN32_FILE_ATTRIBUTE_DATA attrs{};
        if(!GetFileAttributesExW(file.c_str(),GetFileExInfoStandard,&attrs))return false;
        add(&attrs.ftLastWriteTime,sizeof(FILETIME));add(&attrs.nFileSizeHigh,8);
        auto ext=Extension(file);
        if(ext==L".sldasm"||ext==L".sldprt"||ext==L".gltf"||ext==L".obj"){
            // Companion changes invalidate the same-folder image cache as well.
            auto directory=file.substr(0,file.find_last_of(L"\\/"));
            WIN32_FIND_DATAW found{};HANDLE search=FindFirstFileW((directory+L"\\*").c_str(),&found);
            if(search!=INVALID_HANDLE_VALUE){do{
                auto companionExt=Extension(found.cFileName);
                if(found.cFileName[0]==L'~'||(found.dwFileAttributes&FILE_ATTRIBUTE_DIRECTORY))continue;
                bool relevant=(ext==L".sldasm"&&companionExt==L".sldprt")||companionExt==L".png"||companionExt==L".jpg"||companionExt==L".jpeg"||((ext==L".gltf"||ext==L".obj")&&(companionExt==L".bin"||companionExt==L".mtl"));
                if(relevant){add(found.cFileName,wcslen(found.cFileName)*sizeof(wchar_t));add(&found.ftLastWriteTime,sizeof(FILETIME));add(&found.nFileSizeHigh,8);}
            }while(FindNextFileW(search,&found));FindClose(search);}
        }
        // Versioned resolution key never reuses an old 768x512 fallback image.
        wchar_t name[64];swprintf_s(name,L"\\vtx-hd1920-v2-%016llx.png",hash);cache=folder+name;
        wchar_t temporary[96];swprintf_s(temporary,L"\\vtx-hd1920-v2-%016llx-%lu-%p.tmp.png",hash,GetCurrentProcessId(),this);temp=folder+temporary;
        wchar_t dll[MAX_PATH];GetModuleFileNameW(module,dll,MAX_PATH);executable=std::wstring(dll);executable.resize(executable.find_last_of(L"\\/"));executable+=L"\\VTX.exe";
        return true;
    }
    bool Launch(){
        auto command=Quote(executable)+L" --explorer-preview "+std::to_wstring(reinterpret_cast<UINT_PTR>(window))+L" "+Quote(file);
        if(!IsFile(cache))command+=L" "+Quote(temp);
        STARTUPINFOW start{};start.cb=sizeof(start);
        PROCESS_INFORMATION process{};
        if(!CreateProcessW(executable.c_str(),command.data(),nullptr,nullptr,FALSE,CREATE_NO_WINDOW|CREATE_SUSPENDED|BELOW_NORMAL_PRIORITY_CLASS,nullptr,nullptr,&start,&process))return false;
        job=CreateJobObjectW(nullptr,nullptr);
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION limits{};limits.BasicLimitInformation.LimitFlags=JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        if(!job||!SetInformationJobObject(job,JobObjectExtendedLimitInformation,&limits,sizeof(limits))||!AssignProcessToJobObject(job,process.hProcess)){
            TerminateProcess(process.hProcess,ERROR_CANCELLED);CloseHandle(process.hThread);CloseHandle(process.hProcess);if(job)CloseHandle(job);job=nullptr;return false;
        }
        worker=process.hProcess;ResumeThread(process.hThread);CloseHandle(process.hThread);started=GetTickCount64();SetTimer(window,1,15,nullptr);return true;
    }
    bool OwnChild(HWND child){DWORD pid=0;if(!worker||!IsWindow(child)||GetParent(child)!=window)return false;GetWindowThreadProcessId(child,&pid);return pid==GetProcessId(worker);}
    void SizeLive(){auto child=live?live:FindWindowExW(window,nullptr,L"VTXLivePreview",nullptr);if(!OwnChild(child))return;RECT r{};GetClientRect(window,&r);SetWindowPos(child,nullptr,0,48,std::max(1L,r.right),std::max(1L,r.bottom-48),SWP_NOACTIVATE|SWP_NOZORDER);}
    void Interactive(HWND child,bool incomplete=false){
        if(!OwnChild(child))return;live=child;SizeLive();
        partial=incomplete;savedView=partial&&bitmap;
        ShowWindow(live,savedView?SW_HIDE:SW_SHOWNOACTIVATE);SetWindowTextW(window,(L"VTX preview interactive: "+BaseName(file)).c_str());
        SetTimer(window,1,250,nullptr);InvalidateRect(window,nullptr,FALSE);
    }
    void Cached(HWND child){
        if(child!=live||!OwnChild(child))return;
        if(LoadImage(temp)){MoveFileExW(temp.c_str(),cache.c_str(),MOVEFILE_REPLACE_EXISTING);SetWindowTextW(window,(L"VTX preview interactive cached: "+BaseName(file)).c_str());}
    }
    bool Wheel(WPARAM w,LPARAM l){
        if(savedView||!OwnChild(live))return false;
        POINT p={static_cast<short>(LOWORD(l)),static_cast<short>(HIWORD(l))};
        RECT r{};GetClientRect(live,&r);ScreenToClient(live,&p);
        if(!PtInRect(&r,p))return false;
        // Explorer can deliver wheel messages to its preview handler window.
        // Winit consumes the forwarded message, so it cannot bubble back.
        return PostMessageW(live,WM_MOUSEWHEEL,w,l)!=FALSE;
    }
    void Ready(bool sharp=false){status=L"Preview ready";SetWindowTextW(window,((sharp?L"VTX preview sharp: ":L"VTX preview ready: ")+BaseName(file)).c_str());InvalidateRect(window,nullptr,FALSE);}
    void BeginUpgrade(){
        // Timer dispatch happens after the first paint, keeping file/cache work
        // and process startup out of the instant-thumbnail path.
        KillTimer(window,2);
        if(!PrepareCache()){if(!bitmap){status=L"Could not initialize the preview cache.";InvalidateRect(window,nullptr,FALSE);}return;}
        if(IsFile(cache)&&LoadImage(cache))Ready(true);
        if(bitmap)SetTimer(window,3,120,nullptr);
        else if(!Launch()){status=L"Could not start VTX.";InvalidateRect(window,nullptr,FALSE);}
    }
    void Poll(){
        if(!worker)return;
        if(WaitForSingleObject(worker,0)==WAIT_TIMEOUT){
            if(!live){auto child=FindWindowExW(window,nullptr,L"VTXLivePreview",nullptr);if(child&&IsWindowVisible(child))Interactive(child);}
            if(!live&&GetTickCount64()-started>30000){Cancel();if(!bitmap)status=L"Preview took too long.";InvalidateRect(window,nullptr,FALSE);}return;
        }
        Cancel();if(bitmap)Ready(IsFile(cache));else{status=L"No preview available.";InvalidateRect(window,nullptr,FALSE);}
    }
    HBITMAP Scaled(UINT w,UINT h){
        if(w==imageWidth&&h==imageHeight)return bitmap;
        if(scaledBitmap&&scaledWidth==w&&scaledHeight==h)return scaledBitmap;
        ComPtr<IWICBitmap> source;ComPtr<IWICBitmapScaler> scaler;
        if(!Factory()||FAILED(imageFactory->CreateBitmapFromHBITMAP(bitmap,nullptr,WICBitmapUsePremultipliedAlpha,&source))||FAILED(imageFactory->CreateBitmapScaler(&scaler))||FAILED(scaler->Initialize(source.Get(),w,h,WICBitmapInterpolationModeFant)))return bitmap;
        BITMAPINFO info{};info.bmiHeader.biSize=sizeof(BITMAPINFOHEADER);info.bmiHeader.biWidth=w;info.bmiHeader.biHeight=-LONG(h);info.bmiHeader.biPlanes=1;info.bmiHeader.biBitCount=32;info.bmiHeader.biCompression=BI_RGB;
        void* pixels=nullptr;auto next=CreateDIBSection(nullptr,&info,DIB_RGB_COLORS,&pixels,nullptr,0);
        if(!next)return bitmap;
        if(FAILED(scaler->CopyPixels(nullptr,w*4,w*h*4,static_cast<BYTE*>(pixels)))){DeleteObject(next);return bitmap;}
        if(scaledBitmap)DeleteObject(scaledBitmap);scaledBitmap=next;scaledWidth=w;scaledHeight=h;return scaledBitmap;
    }
    void Paint(){
        PAINTSTRUCT ps{};HDC dc=BeginPaint(window,&ps);RECT client{};GetClientRect(window,&client);
        HBRUSH brush=CreateSolidBrush(background);FillRect(dc,&client,brush);DeleteObject(brush);
        auto old=SelectObject(dc,font?font:GetStockObject(DEFAULT_GUI_FONT));SetBkMode(dc,TRANSPARENT);::SetTextColor(dc,text);
        RECT title={10,5,client.right-10,25};auto name=BaseName(file);DrawTextW(dc,name.c_str(),-1,&title,DT_SINGLELINE|DT_END_ELLIPSIS|DT_CENTER);
        RECT hint={8,26,client.right-8,46};auto controls=Extension(file)==L".dxf"?L"MMB/drag pan  |  Scroll zoom  |  F fit":L"MMB rotate  |  Ctrl+MMB pan  |  Scroll zoom";
        if(partial&&bitmap)controls=savedView?L"Saved appearance  |  Click filename for 3D":L"3D  |  Click filename for saved appearance";
        DrawTextW(dc,controls,-1,&hint,DT_SINGLELINE|DT_END_ELLIPSIS|DT_CENTER);
        RECT area={8,48,client.right-8,client.bottom-8};
        if(bitmap&&area.right>area.left&&area.bottom>area.top){
            double scale=std::min(double(area.right-area.left)/imageWidth,double(area.bottom-area.top)/imageHeight);
            int w=std::max(1,int(imageWidth*scale)),h=std::max(1,int(imageHeight*scale));
            auto scaled=Scaled(UINT(w),UINT(h));
            HDC memory=CreateCompatibleDC(dc);auto selected=SelectObject(memory,scaled);
            AlphaBlend(dc,area.left+(area.right-area.left-w)/2,area.top+(area.bottom-area.top-h)/2,w,h,memory,0,0,scaled==bitmap?imageWidth:w,scaled==bitmap?imageHeight:h,{AC_SRC_OVER,0,255,AC_SRC_ALPHA});
            SelectObject(memory,selected);DeleteDC(memory);
        }else DrawTextW(dc,status.c_str(),-1,&area,DT_CENTER|DT_WORDBREAK);
        SelectObject(dc,old);EndPaint(window,&ps);
    }
    static LRESULT CALLBACK WindowProc(HWND hwnd,UINT message,WPARAM w,LPARAM l){
        auto self=reinterpret_cast<Preview*>(GetWindowLongPtrW(hwnd,GWLP_USERDATA));
        if(message==WM_NCCREATE){self=static_cast<Preview*>(reinterpret_cast<CREATESTRUCTW*>(l)->lpCreateParams);self->window=hwnd;SetWindowLongPtrW(hwnd,GWLP_USERDATA,reinterpret_cast<LONG_PTR>(self));}
        if(!self)return DefWindowProcW(hwnd,message,w,l);
        switch(message){
        case WM_PAINT:self->Paint();return 0;
        case WM_ERASEBKGND:return 1;
        case WM_MOUSEWHEEL:if(self->Wheel(w,l))return 0;break;
        case WM_APP+71:self->Interactive(reinterpret_cast<HWND>(w),l!=0);return 0;
        case WM_APP+72:self->Cached(reinterpret_cast<HWND>(w));return 0;
        case WM_LBUTTONUP:if(HIWORD(l)<48&&self->live&&self->bitmap){self->savedView=!self->savedView;ShowWindow(self->live,self->savedView?SW_HIDE:SW_SHOWNOACTIVATE);InvalidateRect(hwnd,nullptr,FALSE);}return 0;
        case WM_TIMER:
            if(w==2)self->BeginUpgrade();
            else if(w==3){KillTimer(hwnd,3);if(!self->Launch()&&!self->bitmap){self->status=L"Could not start VTX.";InvalidateRect(hwnd,nullptr,FALSE);}}
            else self->Poll();return 0;
        case WM_SIZE:self->SizeLive();InvalidateRect(hwnd,nullptr,FALSE);return 0;
        case WM_NCDESTROY:SetWindowLongPtrW(hwnd,GWLP_USERDATA,0);break;
        }
        return DefWindowProcW(hwnd,message,w,l);
    }
public:
    Preview(){InterlockedIncrement(&objects);}
    ~Preview(){Unload();UnregisterClassW(windowClass,module);if(font)DeleteObject(font);InterlockedDecrement(&objects);}
    HRESULT STDMETHODCALLTYPE QueryInterface(REFIID iid,void** out) override{
        if(!out)return E_POINTER;*out=nullptr;
        if(iid==IID_IUnknown||iid==__uuidof(IPreviewHandler))*out=static_cast<IPreviewHandler*>(this);
        else if(iid==__uuidof(IInitializeWithFile))*out=static_cast<IInitializeWithFile*>(this);
        else if(iid==__uuidof(IObjectWithSite))*out=static_cast<IObjectWithSite*>(this);
        else if(iid==__uuidof(IOleWindow))*out=static_cast<IOleWindow*>(this);
        else if(iid==__uuidof(IPreviewHandlerVisuals))*out=static_cast<IPreviewHandlerVisuals*>(this);
        else return E_NOINTERFACE;AddRef();return S_OK;
    }
    ULONG STDMETHODCALLTYPE AddRef() override{return InterlockedIncrement(&references);}
    ULONG STDMETHODCALLTYPE Release() override{auto n=InterlockedDecrement(&references);if(!n)delete this;return n;}
    HRESULT STDMETHODCALLTYPE Initialize(LPCWSTR path,DWORD) override{if(!path)return E_INVALIDARG;if(!file.empty())return HRESULT_FROM_WIN32(ERROR_ALREADY_INITIALIZED);file=path;return S_OK;}
    HRESULT STDMETHODCALLTYPE SetSite(IUnknown* value) override{site=value;return S_OK;}
    HRESULT STDMETHODCALLTYPE GetSite(REFIID iid,void** out) override{return site?site->QueryInterface(iid,out):E_FAIL;}
    HRESULT STDMETHODCALLTYPE SetWindow(HWND value,const RECT* area) override{if(!value||!area)return E_INVALIDARG;parent=value;rect=*area;if(window){::SetParent(window,parent);MoveWindow(window,rect.left,rect.top,rect.right-rect.left,rect.bottom-rect.top,TRUE);}return S_OK;}
    HRESULT STDMETHODCALLTYPE SetRect(const RECT* area) override{if(!area)return E_INVALIDARG;rect=*area;if(window)MoveWindow(window,rect.left,rect.top,rect.right-rect.left,rect.bottom-rect.top,TRUE);return S_OK;}
    HRESULT STDMETHODCALLTYPE DoPreview() override{
        if(!parent||file.empty())return E_UNEXPECTED;
        if(window)return S_OK;
        WNDCLASSEXW wc{};wc.cbSize=sizeof(wc);wc.hInstance=module;wc.lpfnWndProc=WindowProc;wc.lpszClassName=windowClass;wc.hCursor=LoadCursorW(nullptr,IDC_ARROW);RegisterClassExW(&wc);
        window=CreateWindowExW(0,windowClass,(L"VTX preview: "+BaseName(file)).c_str(),WS_CHILD|WS_VISIBLE|WS_CLIPCHILDREN,rect.left,rect.top,rect.right-rect.left,rect.bottom-rect.top,parent,nullptr,module,this);
        if(!window)return HRESULT_FROM_WIN32(GetLastError());
        auto ext=Extension(file);
        if(ext==L".sldprt"||ext==L".sldasm"){
            if(LoadSavedImage())Ready();
        }
        SetTimer(window,2,1,nullptr);
        return S_OK;
    }
    HRESULT STDMETHODCALLTYPE Unload() override{Cancel();if(window)DestroyWindow(window);window=nullptr;if(bitmap)DeleteObject(bitmap);if(scaledBitmap)DeleteObject(scaledBitmap);bitmap=scaledBitmap=nullptr;file.clear();cache.clear();temp.clear();status=L"Loading preview...";return S_OK;}
    HRESULT STDMETHODCALLTYPE SetFocus() override{if(live){auto thread=GetWindowThreadProcessId(live,nullptr);auto current=GetCurrentThreadId();if(AttachThreadInput(current,thread,TRUE)){::SetFocus(live);AttachThreadInput(current,thread,FALSE);}}else if(window)::SetFocus(window);return S_OK;}
    HRESULT STDMETHODCALLTYPE QueryFocus(HWND* out) override{if(!out)return E_POINTER;GUITHREADINFO info{};info.cbSize=sizeof(info);if(live&&GetGUIThreadInfo(GetWindowThreadProcessId(live,nullptr),&info)&&info.hwndFocus==live){*out=live;return S_OK;}*out=::GetFocus();return *out?S_OK:S_FALSE;}
    HRESULT STDMETHODCALLTYPE TranslateAccelerator(MSG* message) override{ComPtr<IPreviewHandlerFrame> frame;if(site&&SUCCEEDED(site.As(&frame)))return frame->TranslateAccelerator(message);return S_FALSE;}
    HRESULT STDMETHODCALLTYPE GetWindow(HWND* out) override{if(!out)return E_POINTER;*out=window;return window?S_OK:E_FAIL;}
    HRESULT STDMETHODCALLTYPE ContextSensitiveHelp(BOOL) override{return E_NOTIMPL;}
    HRESULT STDMETHODCALLTYPE SetBackgroundColor(COLORREF) override{background=RGB(255,255,255);if(window)InvalidateRect(window,nullptr,FALSE);return S_OK;}
    HRESULT STDMETHODCALLTYPE SetTextColor(COLORREF value) override{text=value;if(window)InvalidateRect(window,nullptr,FALSE);return S_OK;}
    HRESULT STDMETHODCALLTYPE SetFont(const LOGFONTW* value) override{if(!value)return E_INVALIDARG;if(font)DeleteObject(font);font=CreateFontIndirectW(value);return S_OK;}
};

class Factory final : public IClassFactory {
    LONG references=1;
public:
    Factory(){InterlockedIncrement(&objects);}~Factory(){InterlockedDecrement(&objects);}
    HRESULT STDMETHODCALLTYPE QueryInterface(REFIID iid,void** out) override{if(!out)return E_POINTER;*out=nullptr;if(iid!=IID_IUnknown&&iid!=IID_IClassFactory)return E_NOINTERFACE;*out=this;AddRef();return S_OK;}
    ULONG STDMETHODCALLTYPE AddRef() override{return InterlockedIncrement(&references);}
    ULONG STDMETHODCALLTYPE Release() override{auto n=InterlockedDecrement(&references);if(!n)delete this;return n;}
    HRESULT STDMETHODCALLTYPE CreateInstance(IUnknown* outer,REFIID iid,void** out) override{if(outer)return CLASS_E_NOAGGREGATION;auto value=new(std::nothrow)Preview;if(!value)return E_OUTOFMEMORY;auto hr=value->QueryInterface(iid,out);value->Release();return hr;}
    HRESULT STDMETHODCALLTYPE LockServer(BOOL lock) override{if(lock)InterlockedIncrement(&objects);else InterlockedDecrement(&objects);return S_OK;}
};
extern "C" HRESULT __stdcall DllGetClassObject(REFCLSID clsid,REFIID iid,void** out){if(clsid!=CLSID_VtxPreview)return CLASS_E_CLASSNOTAVAILABLE;auto factory=new(std::nothrow)Factory;if(!factory)return E_OUTOFMEMORY;auto hr=factory->QueryInterface(iid,out);factory->Release();return hr;}
extern "C" HRESULT __stdcall DllCanUnloadNow(){return objects==0?S_OK:S_FALSE;}
BOOL WINAPI DllMain(HINSTANCE instance,DWORD reason,LPVOID){if(reason==DLL_PROCESS_ATTACH){module=instance;DisableThreadLibraryCalls(instance);}return TRUE;}
