// Windows owns the card surface. This process only supplies JSON and forwards
// explicit device actions to the application's existing single-instance entry.
using System.Diagnostics;
using System.IO.Pipes;
using System.Runtime.InteropServices;
using System.Text.Json;
using Microsoft.Windows.Widgets.Providers;
using WinRT;

namespace ScrcpyAgent.Widget;

internal static class Program
{
    internal const string ClassId = "69BD431E-60CB-493D-A6F6-736B8B37356B";
    internal static readonly ManualResetEvent Quit = new(false);
    private static string ShutdownName => "Local\\ScrcpyAgent.Widget." + Convert.ToHexString(
        System.Security.Cryptography.SHA256.HashData(System.Text.Encoding.UTF8.GetBytes(AppContext.BaseDirectory)))[..16];

    [DllImport("ole32.dll")]
    private static extern int CoRegisterClassObject([MarshalAs(UnmanagedType.LPStruct)] Guid clsid,
        [MarshalAs(UnmanagedType.IUnknown)] object factory, uint context, uint flags, out uint cookie);
    [DllImport("ole32.dll")]
    private static extern int CoRevokeClassObject(uint cookie);

    [MTAThread]
    private static void Main(string[] args)
    {
        if (args is ["--shutdown"])
        {
            if (EventWaitHandle.TryOpenExisting(ShutdownName, out var existing))
                using (existing) existing.Set();
            return;
        }
        using var shutdown = new EventWaitHandle(false, EventResetMode.ManualReset, ShutdownName);
        ComWrappersSupport.InitializeComWrappers();
        var factory = new ProviderFactory();
        Marshal.ThrowExceptionForHR(CoRegisterClassObject(new(ClassId), factory, 4, 1, out var cookie));
        using var refresh = new CancellationTokenSource();
        try
        {
            Provider.Instance.Restore();
            _ = Provider.Instance.Poll(refresh.Token);
            WaitHandle.WaitAny([shutdown, Quit]);
        }
        finally
        {
            refresh.Cancel();
            CoRevokeClassObject(cookie);
            GC.KeepAlive(factory);
        }
    }
}

[ComImport, Guid("00000001-0000-0000-C000-000000000046"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
internal interface IClassFactory
{
    [PreserveSig] int CreateInstance(IntPtr outer, ref Guid iid, out IntPtr result);
    [PreserveSig] int LockServer([MarshalAs(UnmanagedType.Bool)] bool locked);
}

[ComVisible(true)]
internal sealed class ProviderFactory : IClassFactory
{
    public int CreateInstance(IntPtr outer, ref Guid iid, out IntPtr result)
    {
        result = IntPtr.Zero;
        if (outer != IntPtr.Zero) return unchecked((int)0x80040110);
        IntPtr provider = IntPtr.Zero;
        try
        {
            provider = MarshalInspectable<IWidgetProvider>.FromManaged(Provider.Instance);
            return Marshal.QueryInterface(provider, in iid, out result);
        }
        catch (Exception error) { return Marshal.GetHRForException(error); }
        finally { if (provider != IntPtr.Zero) Marshal.Release(provider); }
    }
    public int LockServer(bool locked) => 0;
}

internal sealed record Configuration(string Executable, string Pipe, Dictionary<string, string> Texts);
internal sealed record Snapshot(bool Enabled, WidgetDevice[] Devices, Dictionary<string, string> Texts);
internal sealed record WidgetState(bool Large, bool Active, string? Expanded);

internal sealed class Provider : IWidgetProvider
{
    internal static readonly Provider Instance = new();
    private static readonly JsonSerializerOptions Json = new() { PropertyNameCaseInsensitive = true };
    private readonly object gate = new();
    private readonly Dictionary<string, WidgetState> widgets = new();
    private readonly SemaphoreSlim updating = new(1);

    private static Configuration Config()
    {
        using var file = new FileStream(Path.Combine(AppContext.BaseDirectory, "config.json"),
            FileMode.Open, FileAccess.Read, FileShare.ReadWrite | FileShare.Delete);
        return JsonSerializer.Deserialize<Configuration>(file, Json)
            ?? throw new IOException("The widget configuration is missing.");
    }

    internal void Restore()
    {
        try
        {
            foreach (var info in WidgetManager.GetDefault().GetWidgetInfos())
            {
                var context = info.WidgetContext;
                var state = new WidgetState(context.Size.ToString() == "Large", false, null);
                try { state = state with { Expanded = JsonSerializer.Deserialize<string?>(info.CustomState) }; }
                catch (JsonException) { }
                lock (gate) widgets.TryAdd(context.Id, state);
                Show(context.Id);
            }
        }
        catch (Exception error) { Log(error); }
    }

    public void CreateWidget(WidgetContext context) => SetContext(context, true);
    public void Activate(WidgetContext context) => SetContext(context, true);
    public void OnWidgetContextChanged(WidgetContextChangedArgs args) => SetContext(args.WidgetContext, null);
    private void SetContext(WidgetContext context, bool? active)
    {
        var id = context.Id;
        lock (gate)
        {
            var before = widgets.GetValueOrDefault(id) ?? new(false, false, null);
            widgets[id] = before with { Large = context.Size.ToString() == "Large", Active = active ?? before.Active };
        }
        Show(id);
    }
    public void Deactivate(string id)
    {
        lock (gate)
            if (widgets.TryGetValue(id, out var state)) widgets[id] = state with { Active = false };
    }
    public void DeleteWidget(string id, string customState)
    {
        lock (gate)
        {
            widgets.Remove(id);
            if (widgets.Count == 0) Program.Quit.Set();
        }
    }
    public void OnActionInvoked(WidgetActionInvokedArgs args)
    {
        try
        {
            var id = args.WidgetContext.Id;
            string? device = null;
            if (args.Verb is "device" or "expand" or "mirror")
            {
                using var data = JsonDocument.Parse(args.Data);
                if (data.RootElement.TryGetProperty("device", out var value)) device = value.GetString();
                if (string.IsNullOrEmpty(device)) return;
            }
            if (args.Verb == "expand")
            {
                lock (gate)
                    if (widgets.TryGetValue(id, out var state))
                        widgets[id] = state with { Expanded = state.Expanded == device ? null : device };
            }
            else if (args.Verb is "open" or "device" or "mirror")
            {
                var start = new ProcessStartInfo(Config().Executable) { UseShellExecute = true };
                if (args.Verb != "open")
                {
                    start.ArgumentList.Add("--widget");
                    start.ArgumentList.Add(args.Verb);
                    start.ArgumentList.Add(device!);
                }
                Process.Start(start);
            }
            Show(id);
        }
        catch (Exception error) { Log(error); }
    }

    internal async Task Poll(CancellationToken cancellation)
    {
        using var timer = new PeriodicTimer(TimeSpan.FromSeconds(5));
        try
        {
            while (await timer.WaitForNextTickAsync(cancellation))
            {
                string[] active;
                lock (gate) active = widgets.Where(item => item.Value.Active).Select(item => item.Key).ToArray();
                foreach (var id in active) await Update(id);
            }
        }
        catch (OperationCanceledException) { }
        catch (Exception error) { Log(error); }
    }
    private async void Show(string id)
    {
        try { await Update(id); }
        catch (Exception error) { Log(error); }
    }
    private async Task Update(string id)
    {
        await updating.WaitAsync();
        try
        {
            var config = Config();
            var snapshot = new Snapshot(true, [], config.Texts);
            var running = false;
            try
            {
                using var pipe = new NamedPipeClientStream(".", config.Pipe, PipeDirection.In,
                    PipeOptions.Asynchronous | PipeOptions.CurrentUserOnly);
                using var timeout = new CancellationTokenSource(1500);
                await pipe.ConnectAsync(timeout.Token);
                snapshot = await JsonSerializer.DeserializeAsync<Snapshot>(pipe, Json, timeout.Token) ?? snapshot;
                running = true;
            }
            catch (Exception error) when (error is IOException or OperationCanceledException) { }
            if (!snapshot.Enabled) { Program.Quit.Set(); return; }
            WidgetState state;
            lock (gate)
            {
                if (!widgets.TryGetValue(id, out state!)) return;
                if (!snapshot.Devices.Any(device => device.Id == state.Expanded))
                    widgets[id] = state = state with { Expanded = null };
            }
            WidgetManager.GetDefault().UpdateWidget(new WidgetUpdateRequestOptions(id) {
                Template = Card.Build(snapshot.Devices, running, state.Large, state.Expanded, snapshot.Texts),
                Data = "{}", CustomState = JsonSerializer.Serialize(state.Expanded)
            });
        }
        catch (Exception error) { Log(error); }
        finally { updating.Release(); }
    }

    private static void Log(Exception error)
    {
        // Keep diagnostics bounded, beside this deployment, never beside the app executable.
        try
        {
            var path = Path.Combine(AppContext.BaseDirectory, "widget.log");
            if (File.Exists(path) && new FileInfo(path).Length > 128 * 1024) File.WriteAllText(path, "");
            File.AppendAllText(path, $"{DateTimeOffset.Now:O} {error}{Environment.NewLine}");
        }
        catch (IOException) { }
        catch (UnauthorizedAccessException) { }
    }
}
