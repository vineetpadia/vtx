#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <shobjidl.h>
#include <wrl/client.h>
#include <chrono>
#include <iostream>
#include <tlhelp32.h>
#include <cmath>
#include <cstring>
using Microsoft::WRL::ComPtr;
static const CLSID id={0x75660313,0x4b6c,0x45d7,{0x96,0xd9,0x39,0x60,0x8f,0x52,0xf0,0x40}};
static void Pump(){MSG msg;while(PeekMessageW(&msg,nullptr,0,0,PM_REMOVE)){TranslateMessage(&msg);DispatchMessageW(&msg);}}
static int Workers(){int n=0;auto snapshot=CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS,0);PROCESSENTRY32W p{};p.dwSize=sizeof(p);if(Process32FirstW(snapshot,&p))do{if(p.th32ParentProcessID==GetCurrentProcessId()&&!_wcsicmp(p.szExeFile,L"VTX.exe"))++n;}while(Process32NextW(snapshot,&p));CloseHandle(snapshot);return n;}
int wmain(int argc,wchar_t** argv){
    if(argc<3)return 2;
    if(!wcscmp(argv[2],L"--dpi"))SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    CoInitializeEx(nullptr,COINIT_APARTMENTTHREADED);
    auto module=LoadLibraryW(argv[1]);if(!module)return 3;
    auto create=reinterpret_cast<HRESULT(__stdcall*)(REFCLSID,REFIID,void**)>(GetProcAddress(module,"DllGetClassObject"));
    ComPtr<IClassFactory> factory;if(FAILED(create(id,IID_PPV_ARGS(&factory))))return 4;
    WNDCLASSW cls{};cls.lpfnWndProc=DefWindowProcW;cls.hInstance=GetModuleHandleW(nullptr);cls.lpszClassName=L"VTXPreviewTestHost";RegisterClassW(&cls);
    auto host=CreateWindowW(cls.lpszClassName,L"VTX preview test host",WS_OVERLAPPEDWINDOW,0,0,700,600,nullptr,nullptr,cls.hInstance,nullptr);
    ComPtr<IPreviewHandler> preview;if(FAILED(factory->CreateInstance(nullptr,IID_PPV_ARGS(&preview))))return 5;
    ComPtr<IInitializeWithFile> input;preview.As(&input);
    int firstArg=2;bool instantOnly=false,cancelBefore=false,cancelAfter=false,retain=false,liveOnly=false,show=false,saved=false,wheel=false;
    if(!wcscmp(argv[2],L"--instant")){instantOnly=true;++firstArg;}
    if(!wcscmp(argv[2],L"--cancel-before")){cancelBefore=true;++firstArg;}
    if(!wcscmp(argv[2],L"--cancel-after")){cancelAfter=true;++firstArg;}
    if(!wcscmp(argv[2],L"--retain")){instantOnly=retain=true;++firstArg;}
    if(!wcscmp(argv[2],L"--live")){liveOnly=true;++firstArg;}
    if(!wcscmp(argv[2],L"--saved")){saved=liveOnly=true;++firstArg;}
    if(!wcscmp(argv[2],L"--wheel")){wheel=liveOnly=true;++firstArg;ShowWindow(host,SW_SHOW);}
    if(!wcscmp(argv[2],L"--dpi"))++firstArg;
    if(!wcscmp(argv[2],L"--show")){show=liveOnly=true;++firstArg;ShowWindow(host,SW_SHOW);}
    for(int arg=firstArg;arg<argc;++arg){
        for(int trial=0;trial<3;++trial){
            auto begin=std::chrono::steady_clock::now();
            if(FAILED(input->Initialize(argv[arg],STGM_READ)))return 6;
            RECT area={0,0,640,480};preview->SetWindow(host,&area);
            if(FAILED(preview->DoPreview()))return 7;
            auto submit=std::chrono::steady_clock::now();
            auto child=FindWindowExW(host,nullptr,L"VTXExplorerPreview",nullptr);if(!child)return 10;
            bool ready=false,sharp=false,interactive=false;double firstMs=-1,liveMs=-1;
            auto observe=[&](){wchar_t title[1024]{};GetWindowTextW(child,title,1024);sharp=sharp||wcsstr(title,L"VTX preview sharp:")||wcsstr(title,L"VTX preview interactive cached:");if(wcsstr(title,L"VTX preview interactive")){if(!interactive)liveMs=std::chrono::duration<double,std::milli>(std::chrono::steady_clock::now()-begin).count();interactive=true;}if(sharp||interactive||wcsstr(title,L"VTX preview ready:")){if(!ready){UpdateWindow(child);firstMs=std::chrono::duration<double,std::milli>(std::chrono::steady_clock::now()-begin).count();}ready=true;}};
            observe();
            if(cancelBefore||cancelAfter){
                if(cancelAfter){auto until=std::chrono::steady_clock::now()+std::chrono::seconds(3);while(!Workers()&&std::chrono::steady_clock::now()<until){Pump();Sleep(1);}if(!Workers())return 11;}
                else if(Workers())return 12;
                preview->Unload();Sleep(30);if(Workers())return 13;
                std::wcout<<L"PASS cancel_"<<(cancelBefore?L"before":L"after")<<L"_launch first_ms="<<firstMs<<std::endl;break;
            }
            while(std::chrono::steady_clock::now()-begin<std::chrono::seconds(35)){
                if((instantOnly&&ready&&(!retain||std::chrono::steady_clock::now()-begin>std::chrono::seconds(1)))||(!instantOnly&&interactive&&(liveOnly||sharp)))break;
                Pump();observe();Sleep(1);
            }
            auto done=std::chrono::steady_clock::now();
            if(FindWindowExW(host,nullptr,L"VTXExplorerPreview",nullptr)!=child||FindWindowExW(child,nullptr,L"BUTTON",nullptr))return 14;
            if(retain&&(!ready||sharp||Workers()))return 15;
            std::wcout<<L"trial="<<trial<<L" submit_ms="<<std::chrono::duration<double,std::milli>(submit-begin).count()<<L" first_ms="<<firstMs<<L" live_ms="<<liveMs<<L" sharp_ms="<<std::chrono::duration<double,std::milli>(done-begin).count()<<L" ready="<<ready<<L" sharp="<<sharp<<L" interactive="<<interactive<<L" file="<<argv[arg]<<std::endl;
            auto live=FindWindowExW(child,nullptr,L"VTXLivePreview",nullptr);
            DWORD pid=0;if(live)GetWindowThreadProcessId(live,&pid);
            if(interactive&&(!live||GetParent(live)!=child||!(GetWindowLongPtrW(live,GWL_STYLE)&WS_CHILD)||Workers()!=1))return 16;
            if(wheel){
                // Reproduce Explorer leaving focus outside the GPU child.
                SetForegroundWindow(host);::SetFocus(host);
                bool active=GetForegroundWindow()==host;
                if(!active)std::wcout<<L"SKIP hover-focus assertion: Windows denied test-host activation"<<std::endl;
                SendMessageW(live,WM_MOUSEMOVE,0,MAKELPARAM(160,180));
                auto until=std::chrono::steady_clock::now()+std::chrono::seconds(2);
                GUITHREADINFO info{};info.cbSize=sizeof(info);
                while(active&&std::chrono::steady_clock::now()<until){Pump();GetGUIThreadInfo(GetWindowThreadProcessId(live,nullptr),&info);if(info.hwndFocus==live)break;Sleep(5);}
                if(active&&info.hwndFocus!=live){std::wcout<<L"FAIL: hovering leaves wheel focus outside the live preview"<<std::endl;return 25;}
                auto zoom=[&](){auto bits=static_cast<UINT32>(reinterpret_cast<UINT_PTR>(GetPropW(live,L"VTX.PresentedZoom")));float value=0;std::memcpy(&value,&bits,sizeof(value));return value;};
                auto waitZoom=[&](float expected){auto until=std::chrono::steady_clock::now()+std::chrono::seconds(2);while(std::chrono::steady_clock::now()<until){Pump();if(std::abs(zoom()-expected)<0.0001f)return true;Sleep(5);}return false;};
                float before=zoom();if(!(before>0))return 27;
                POINT p={160,180};ClientToScreen(live,&p);auto point=MAKELPARAM(p.x,p.y);
                auto target=active?info.hwndFocus:live;
                SendMessageW(target,WM_MOUSEWHEEL,MAKEWPARAM(0,-WHEEL_DELTA),point);
                if(!waitZoom(before*std::exp(0.12f))){std::wcout<<L"FAIL: focused wheel did not present a zoomed frame before="<<before<<L" after="<<zoom()<<std::endl;return 28;}
                SendMessageW(target,WM_MOUSEWHEEL,MAKEWPARAM(0,WHEEL_DELTA),point);
                if(!waitZoom(before))return 29;
                // Handler-level wheel routing also zooms once, without a click.
                ::SetFocus(child);SendMessageW(child,WM_MOUSEWHEEL,MAKEWPARAM(0,-WHEEL_DELTA),point);
                if(!waitZoom(before*std::exp(0.12f)))return 30;
                Sleep(50);Pump();if(std::abs(zoom()-before*std::exp(0.12f))>=0.0001f)return 31;
                SendMessageW(child,WM_MOUSEWHEEL,MAKEWPARAM(0,WHEEL_DELTA),point);if(!waitZoom(before))return 32;
                POINT header={10,10};ClientToScreen(child,&header);SendMessageW(child,WM_MOUSEWHEEL,MAKEWPARAM(0,-WHEEL_DELTA),MAKELPARAM(header.x,header.y));
                Sleep(50);Pump();if(std::abs(zoom()-before)>=0.0001f)return 33;
                auto other=CreateWindowW(cls.lpszClassName,L"VTX inactive-preview test",WS_OVERLAPPEDWINDOW,20,20,260,150,nullptr,nullptr,cls.hInstance,nullptr);
                ShowWindow(other,SW_SHOW);SetForegroundWindow(other);::SetFocus(other);
                if(GetForegroundWindow()==other){
                    SendMessageW(live,WM_MOUSEMOVE,0,MAKELPARAM(180,180));Sleep(50);Pump();
                    if(GetForegroundWindow()!=other||::GetFocus()!=other){std::wcout<<L"FAIL: inactive preview stole focus"<<std::endl;return 34;}
                }else std::wcout<<L"SKIP inactive-window assertion: Windows denied second test-host activation"<<std::endl;
                DestroyWindow(other);SetForegroundWindow(host);
                std::wcout<<L"PASS: plain wheel in/out, rendered zoom, parent routing once, header exclusion; hover_focus_checked="<<active<<std::endl;
            }
            if(saved){
                if(!live||(GetWindowLongPtrW(live,GWL_STYLE)&WS_VISIBLE))return 22;
                SendMessageW(child,WM_LBUTTONUP,0,MAKELPARAM(10,10));
                if(!(GetWindowLongPtrW(live,GWL_STYLE)&WS_VISIBLE))return 23;
                SendMessageW(child,WM_LBUTTONUP,0,MAKELPARAM(10,10));
                if(GetWindowLongPtrW(live,GWL_STYLE)&WS_VISIBLE)return 24;
            }
            // Winit's internal dispatcher uses WS_VISIBLE but isn't an app
            // window. Reject every other visible top-level worker window.
            if(pid){bool top=false;struct Check{DWORD pid;bool* top;} check{pid,&top};EnumWindows([](HWND h,LPARAM p)->BOOL{auto c=reinterpret_cast<Check*>(p);DWORD id=0;GetWindowThreadProcessId(h,&id);if(id==c->pid&&IsWindowVisible(h)){wchar_t name[128]{};GetClassNameW(h,name,128);if(wcscmp(name,L"Winit Thread Event Target"))*c->top=true;}return TRUE;},reinterpret_cast<LPARAM>(&check));if(top)return 17;}
            RECT resized={0,0,320,360};preview->SetRect(&resized);Pump();UpdateWindow(child);
            if(live){RECT actual{};GetClientRect(live,&actual);if(actual.right!=320||actual.bottom!=312)return 18;}
            if(live){
                // Exercise the real child event loop, including orbit, pan,
                // wheel and reset. Camera-space results have Rust unit tests.
                struct Input{UINT message;WPARAM w;LPARAM l;};
                Input events[]={{WM_MOUSEMOVE,0,MAKELPARAM(100,100)},{WM_MBUTTONDOWN,MK_MBUTTON,MAKELPARAM(100,100)},
                    {WM_MOUSEMOVE,MK_MBUTTON,MAKELPARAM(200,140)},{WM_MBUTTONUP,0,MAKELPARAM(200,140)},
                    {WM_MBUTTONDOWN,MK_MBUTTON|MK_CONTROL,MAKELPARAM(200,140)},{WM_MOUSEMOVE,MK_MBUTTON|MK_CONTROL,MAKELPARAM(230,160)},
                    {WM_MBUTTONUP,0,MAKELPARAM(230,160)},{WM_MOUSEWHEEL,MAKEWPARAM(0,WHEEL_DELTA),0},
                    {WM_KEYDOWN,'F',1|(MapVirtualKeyW('F',MAPVK_VK_TO_VSC)<<16)},{WM_KEYUP,'F',1|(MapVirtualKeyW('F',MAPVK_VK_TO_VSC)<<16)|(1LL<<30)|(1LL<<31)}};
                for(auto event:events){DWORD_PTR result=0;if(!SendMessageTimeoutW(live,event.message,event.w,event.l,SMTO_ABORTIFHUNG,2000,&result))return 20;Pump();}
                Sleep(50);Pump();if(Workers()!=1||!IsWindow(live))return 21;
            }
            if(show){auto until=std::chrono::steady_clock::now()+std::chrono::seconds(90);while(std::chrono::steady_clock::now()<until&&IsWindow(host)){Pump();Sleep(10);}}
            preview->Unload();if(live&&IsWindow(live))return 19;if(Workers())return 13;if(!ready||(!instantOnly&&(!interactive||(!liveOnly&&!sharp))))return 8;
        }
    }
    input.Reset();preview.Reset();factory.Reset();DestroyWindow(host);
    auto unload=reinterpret_cast<HRESULT(__stdcall*)()>(GetProcAddress(module,"DllCanUnloadNow"));
    if(unload()!=S_OK)return 9;
    FreeLibrary(module);CoUninitialize();return 0;
}
