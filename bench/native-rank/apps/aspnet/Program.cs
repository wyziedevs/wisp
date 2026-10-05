using System.Net;
using System.Text;

var builder = WebApplication.CreateBuilder(args);
builder.Logging.ClearProviders();
builder.WebHost.ConfigureKestrel(o => o.AddServerHeader = false);
builder.WebHost.UseUrls("http://0.0.0.0:" + (Environment.GetEnvironmentVariable("PORT") ?? "8080"));
var app = builder.Build();

app.MapGet("/", () => "Hello, World!");
app.MapGet("/json", () => new Msg("Hello, World!"));
app.MapGet("/params/{id}", (string id, string? q, HttpRequest r) =>
    $"id={id} q={q ?? ""} sid={(r.Cookies.TryGetValue("sid", out var s) ? s : "none")}");
app.MapGet("/list", () =>
{
    var sb = new StringBuilder(48 * 1024);
    sb.Append("<!DOCTYPE html><html><head></head><body><h1>List</h1><ul>");
    for (var i = 1; i <= 1000; i++) sb.Append("<li>").Append(WebUtility.HtmlEncode($"Item <{i}> & co")).Append("</li>");
    sb.Append("</ul></body></html>");
    return Results.Content(sb.ToString(), "text/html; charset=utf-8");
});
app.MapGet("/json-big", () =>
{
    var rows = new Row[200];
    for (var i = 0; i < 200; i++)
    {
        var n = i + 1;
        rows[i] = new Row(n, $"user-{n}", n % 3 != 0, n * 37 % 101, ["a", $"t{n % 7}"]);
    }
    return rows;
});
app.Run();

record Msg(string message);
record Row(int id, string name, bool active, int score, string[] tags);
