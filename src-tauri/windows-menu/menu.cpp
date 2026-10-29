// Windows 11 asks this small COM server about the menu. Device discovery, file
// transfers and windows stay in the application; Invoke only passes file names.
#define WIN32_LEAN_AND_MEAN
#define NOMINMAX
#include <windows.h>
#include <shobjidl.h>
#include <shellapi.h>
#include <shlwapi.h>
#include <wrl.h>
#include <atomic>
#include <string>
#include <vector>

using namespace Microsoft::WRL;

// Also written in the package manifest in core/integration/windows11_menu.rs.
static const CLSID CLSID_Menu =
    {0xd4aa43e6, 0x08cb, 0x43bf, {0x89, 0x1d, 0xe3, 0x6c, 0x27, 0x0f, 0xf8, 0xd1}};
static constexpr wchar_t CONFIG[] = L"Software\\scrcpy-agent\\Windows11Menu";
static constexpr const wchar_t* ACTIONS[] =
    {L"send-default", L"send-choose", L"open-default", L"open-choose"};
static constexpr const wchar_t* LABELS[] =
    {L"SendDefault", L"SendChoose", L"OpenDefault", L"OpenChoose"};
static std::atomic<long> objects{0};

static std::wstring setting(const wchar_t* name) {
    DWORD bytes = 0;
    if (RegGetValueW(HKEY_CURRENT_USER, CONFIG, name, RRF_RT_REG_SZ, nullptr, nullptr, &bytes) != ERROR_SUCCESS
        || bytes < sizeof(wchar_t) || bytes > 65536) return {};
    std::vector<wchar_t> value(bytes / sizeof(wchar_t) + 1, L'\0');
    if (RegGetValueW(HKEY_CURRENT_USER, CONFIG, name, RRF_RT_REG_SZ, nullptr, value.data(), &bytes) != ERROR_SUCCESS) return {};
    return value.data();
}

// Windows command-line quoting, including paths ending with a backslash.
static std::wstring quote(const std::wstring& value) {
    std::wstring result = L"\"";
    size_t slashes = 0;
    for (wchar_t ch : value) {
        if (ch == L'\\') { ++slashes; continue; }
        result.append(ch == L'"' ? slashes * 2 + 1 : slashes, L'\\');
        result += ch;
        slashes = 0;
    }
    result.append(slashes * 2, L'\\');
    return result + L'"';
}

class Counted {
public:
    Counted() { ++objects; }
    ~Counted() { --objects; }
};

class Command;
class Commands final : public RuntimeClass<RuntimeClassFlags<ClassicCom>, IEnumExplorerCommand>, private Counted {
    ULONG next_ = 0;
public:
    explicit Commands(ULONG next = 0) : next_(next) {}
    IFACEMETHODIMP Next(ULONG count, IExplorerCommand** commands, ULONG* fetched) override;
    IFACEMETHODIMP Skip(ULONG count) override {
        const ULONG remaining = 4 - next_;
        next_ += count < remaining ? count : remaining;
        return count <= remaining ? S_OK : S_FALSE;
    }
    IFACEMETHODIMP Reset() override { next_ = 0; return S_OK; }
    IFACEMETHODIMP Clone(IEnumExplorerCommand** result) override {
        auto copy = Make<Commands>(next_);
        return copy ? copy.CopyTo(result) : E_OUTOFMEMORY;
    }
};

class Command final : public RuntimeClass<RuntimeClassFlags<ClassicCom>, IExplorerCommand>, private Counted {
    int action_;
public:
    explicit Command(int action = -1) : action_(action) {}
    IFACEMETHODIMP GetTitle(IShellItemArray*, LPWSTR* result) override {
        return SHStrDupW(setting(action_ < 0 ? L"Title" : LABELS[action_]).c_str(), result);
    }
    IFACEMETHODIMP GetIcon(IShellItemArray*, LPWSTR* result) override {
        return SHStrDupW((quote(setting(L"Executable")) + L",0").c_str(), result);
    }
    IFACEMETHODIMP GetToolTip(IShellItemArray*, LPWSTR* result) override {
        *result = nullptr;
        return E_NOTIMPL;
    }
    IFACEMETHODIMP GetCanonicalName(GUID* result) override {
        *result = CLSID_Menu;
        result->Data1 += static_cast<ULONG>(action_ + 1);
        return S_OK;
    }
    IFACEMETHODIMP GetState(IShellItemArray* items, BOOL, EXPCMDSTATE* result) override {
        *result = ECS_HIDDEN;
        if (!items || setting(L"Executable").empty()) return S_OK;
        DWORD count = 0;
        if (FAILED(items->GetCount(&count)) || count == 0) return S_OK;
        SFGAOF attributes = 0;
        if (SUCCEEDED(items->GetAttributes(SIATTRIBFLAGS_AND, SFGAO_FILESYSTEM, &attributes))
            && (attributes & SFGAO_FILESYSTEM)) {
            // All selected items must be files. Mixed file/folder selections stay hidden.
            for (DWORD i = 0; i < count; ++i) {
                ComPtr<IShellItem> item;
                if (FAILED(items->GetItemAt(i, &item))
                    || FAILED(item->GetAttributes(SFGAO_FOLDER, &attributes))
                    || (attributes & SFGAO_FOLDER)) return S_OK;
            }
            *result = ECS_ENABLED;
        }
        return S_OK;
    }
    IFACEMETHODIMP GetFlags(EXPCMDFLAGS* result) override {
        *result = action_ < 0 ? ECF_HASSUBCOMMANDS : ECF_DEFAULT;
        return S_OK;
    }
    IFACEMETHODIMP EnumSubCommands(IEnumExplorerCommand** result) override {
        *result = nullptr;
        if (action_ >= 0) return E_NOTIMPL;
        auto commands = Make<Commands>();
        return commands ? commands.CopyTo(result) : E_OUTOFMEMORY;
    }
    IFACEMETHODIMP Invoke(IShellItemArray* items, IBindCtx*) override {
        if (action_ < 0 || !items) return E_INVALIDARG;
        const auto executable = setting(L"Executable");
        if (executable.empty()) return HRESULT_FROM_WIN32(ERROR_FILE_NOT_FOUND);
        const std::wstring prefix = std::wstring(L"--shell ") + ACTIONS[action_];
        std::wstring arguments = prefix;
        auto launch = [&]() -> HRESULT {
            SHELLEXECUTEINFOW execute{};
            execute.cbSize = static_cast<DWORD>(sizeof(execute));
            execute.fMask = SEE_MASK_FLAG_NO_UI;
            execute.lpFile = executable.c_str();
            execute.lpParameters = arguments.c_str();
            execute.nShow = SW_SHOWNORMAL;
            return ShellExecuteExW(&execute) ? S_OK : HRESULT_FROM_WIN32(GetLastError());
        };
        DWORD count = 0;
        HRESULT hr = items->GetCount(&count);
        if (FAILED(hr) || count == 0) return FAILED(hr) ? hr : E_INVALIDARG;
        for (DWORD i = 0; i < count; ++i) {
            ComPtr<IShellItem> item;
            hr = items->GetItemAt(i, &item);
            if (FAILED(hr)) return hr;
            PWSTR path = nullptr;
            hr = item->GetDisplayName(SIGDN_FILESYSPATH, &path);
            if (FAILED(hr)) return hr;
            const auto argument = L" " + quote(path);
            CoTaskMemFree(path);
            // Keep well below CreateProcess's limit. The existing inbox joins batches.
            if (arguments.size() + argument.size() > 30000) {
                if (arguments == prefix) return HRESULT_FROM_WIN32(ERROR_FILENAME_EXCED_RANGE);
                hr = launch();
                if (FAILED(hr)) return hr;
                arguments = prefix;
            }
            arguments += argument;
        }
        return launch();
    }
};

HRESULT Commands::Next(ULONG count, IExplorerCommand** commands, ULONG* fetched) {
    if (!commands || (!fetched && count != 1)) return E_POINTER;
    ULONG made = 0;
    for (; made < count && next_ < 4; ++made, ++next_) {
        auto command = Make<Command>(static_cast<int>(next_));
        if (!command) return E_OUTOFMEMORY;
        command.CopyTo(&commands[made]);
    }
    if (fetched) *fetched = made;
    return made == count ? S_OK : S_FALSE;
}

class Factory final : public RuntimeClass<RuntimeClassFlags<ClassicCom>, IClassFactory>, private Counted {
public:
    IFACEMETHODIMP CreateInstance(IUnknown* outer, REFIID iid, void** result) override {
        if (!result) return E_POINTER;
        *result = nullptr;
        if (outer) return CLASS_E_NOAGGREGATION;
        auto command = Make<Command>();
        return command ? command.CopyTo(iid, result) : E_OUTOFMEMORY;
    }
    IFACEMETHODIMP LockServer(BOOL lock) override {
        if (lock) ++objects; else --objects;
        return S_OK;
    }
};

extern "C" HRESULT __stdcall DllGetClassObject(REFCLSID clsid, REFIID iid, void** result) {
    if (!result) return E_POINTER;
    *result = nullptr;
    if (clsid != CLSID_Menu) return CLASS_E_CLASSNOTAVAILABLE;
    auto factory = Make<Factory>();
    return factory ? factory.CopyTo(iid, result) : E_OUTOFMEMORY;
}

extern "C" HRESULT __stdcall DllCanUnloadNow() {
    return objects == 0 ? S_OK : S_FALSE;
}
