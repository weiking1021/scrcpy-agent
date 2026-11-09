using System.Text.Json;

namespace ScrcpyAgent.Widget;

internal sealed record WidgetDevice(string Id, string Name, int? Battery, bool Charging);

internal static class Card
{
    private static readonly Dictionary<string, string> Icons = new[] {
        "device", "screen", "info", "chevron-right", "chevron-down"
    }.SelectMany(name => new[] { name, name + ".light" }).ToDictionary(name => name, name => {
        using var stream = typeof(Card).Assembly.GetManifestResourceStream($"ScrcpyAgent.Widget.icons.{name}.png")
            ?? throw new InvalidOperationException($"Missing widget icon: {name}");
        using var bytes = new MemoryStream();
        stream.CopyTo(bytes);
        return "data:image/png;base64," + Convert.ToBase64String(bytes.ToArray());
    });

    private static object Icon(string name, int size = 20) => new {
        type = "Image", url = "${if($host.hostTheme == 'light', '" + Icons[name + ".light"] + "', '" + Icons[name] + "')}", width = $"{size}px", height = $"{size}px", altText = ""
    };

    private static object Text(string text, string size = "Default", string color = "Default",
        string weight = "Default", bool subtle = false, string alignment = "Left", int maxLines = 1) => new {
        type = "TextBlock", text, size, color, weight, isSubtle = subtle,
        spacing = "None", wrap = true, maxLines, horizontalAlignment = alignment
    };

    private static object Column(string width, params object[] items) => new {
        type = "Column", width, verticalContentAlignment = "Center", items
    };

    private static object ActionTile(string icon, string title, string verb, string device) => new {
        type = "Column", width = "stretch", style = "default", minHeight = "48px",
        verticalContentAlignment = "Center",
        selectAction = new { type = "Action.Execute", verb, data = new { device } },
        items = new object[] {
            new {
                type = "ColumnSet", spacing = "None",
                columns = new[] {
                    Column("12px"),
                    Column("20px", Icon(icon)),
                    Column("stretch", Text(title, "Small", weight: "Bolder", maxLines: 2)),
                    Column("12px")
                }
            }
        }
    };

    public static string Build(WidgetDevice[] devices, bool running, bool large, string? expanded, IReadOnlyDictionary<string, string> texts)
    {
        string T(string key) => texts[key];
        var selected = devices.FirstOrDefault(device => device.Id == expanded);
        var limit = (large ? 5 : 3) - (selected is null ? 0 : 1);
        var visible = devices.Take(limit).ToList();
        if (selected is not null && !visible.Contains(selected)) visible[^1] = selected;
        var body = new List<object> {
            new {
                type = "ColumnSet", spacing = "None",
                columns = new[] {
                    Column("stretch",
                        Text(T("overview"), "Medium", weight: "Bolder"),
                        Text(running ? T("service_running") : T("service_stopped"), "Small",
                            running ? "Good" : "Default", subtle: !running)),
                    Column("auto", Text(devices.Length.ToString("00"), "ExtraLarge", "Accent", "Bolder"),
                        Text(T("online"), "Small", subtle: true))
                }
            }
        };

        var rows = new List<object>();
        foreach (var device in visible)
        {
            var isExpanded = device.Id == expanded;
            var items = new List<object> {
                new {
                    type = "ColumnSet", spacing = "None",
                    selectAction = new { type = "Action.Execute", verb = "expand", data = new { device = device.Id } },
                    columns = new[] {
                        Column("24px", Icon("device", 24)),
                        Column("stretch", Text(device.Name, weight: "Bolder"),
                            Text(T("online"), "Small", "Good")),
                        Column("64px", Text(device.Battery is int percent ? $"{percent}%" : "—", color:
                                device.Battery is <= 20 ? "Warning" : "Default", weight: "Bolder", alignment: "Right"),
                            Text(device.Charging ? T("charging") : device.Battery is null ? T("battery_unknown") : T("battery"), "Small", subtle: true, alignment: "Right")),
                        Column("16px", Icon(isExpanded ? "chevron-down" : "chevron-right", 16))
                    }
                }
            };
            if (isExpanded)
                items.Add(new {
                    type = "ColumnSet", spacing = "Small",
                    columns = new[] {
                        ActionTile("screen", T("mirror"), "mirror", device.Id),
                        ActionTile("info", T("details"), "device", device.Id)
                    }
                });
            rows.Add(new {
                type = "Container", separator = rows.Count > 0, spacing = "Small",
                items
            });
        }
        if (rows.Count > 0)
            body.Add(new { type = "Container", style = "emphasis", spacing = "Medium", items = rows });

        if (devices.Length == 0)
        {
            body.Add(new {
                type = "Container", style = "emphasis", spacing = "Medium",
                items = new object[] {
                    Text(running ? T("wait_title") : T("start_title"), "Medium", weight: "Bolder"),
                    new { type = "TextBlock", text = running
                        ? T("wait_hint")
                        : T("start_hint"),
                        isSubtle = true, wrap = true, spacing = "Small" }
                }
            });
        }

        if (devices.Length > limit)
            body.Add(new {
                type = "TextBlock", size = "Small", isSubtle = true, wrap = true,
                spacing = "Small", text = T("more").Replace("{count}", (devices.Length - limit).ToString())
            });

        var card = new Dictionary<string, object> {
            ["type"] = "AdaptiveCard", ["version"] = "1.5", ["body"] = body
        };
        if (!running)
            card["actions"] = new[] { new { type = "Action.Execute", title = T("open"), verb = "open" } };
        return JsonSerializer.Serialize(card);
    }
}
