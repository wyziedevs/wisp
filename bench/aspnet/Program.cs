using System.Net.WebSockets;
using System.Text.Encodings.Web;
using System.Text.Json;
using System.Text.Unicode;
using Microsoft.AspNetCore.Http.HttpResults;
using Microsoft.Extensions.WebEncoders;
using Bench.Components;

var builder = WebApplication.CreateBuilder(args);
// The project templates set this in appsettings.json; without it every
// request is logged to the console.
builder.Logging.SetMinimumLevel(LogLevel.Warning);
builder.Services.AddRazorPages();
builder.Services.AddRazorComponents();
// Emit non-ASCII text as is (the default encodes it as &#x..;), matching
// Wisp's output and TechEmpower's configuration.
builder.Services.Configure<WebEncoderOptions>(o => o.TextEncoderSettings = new TextEncoderSettings(UnicodeRanges.All));

builder.WebHost.ConfigureKestrel(o => o.Limits.MaxRequestBodySize = 8 * 1024 * 1024);

var app = builder.Build();
app.UseStaticFiles(); // wwwroot/static/app.js, linked from ../static in Bench.csproj
app.UseWebSockets();
app.MapGet("/plaintext", () => "Hello, World!");
app.MapGet("/json", () => new Message("Hello, World!"));
app.MapRazorPages(); // /fortunes and /page
app.MapGet("/fortunes-blazor", () => new RazorComponentResult<FortunesPage>(new { Fortunes = Fortune.Load() }));
// the-benchmarker's routes, as its aspnet-minimal-api entry maps them.
app.MapGet("/", () => { });
app.MapGet("user/{id}", (string id) => id);
app.MapPost("user", () => { });
// The practice routes (README): a wait, a validated body, an upload, a list
// and a WebSocket; the static file is above.
app.MapGet("/wait", async () =>
{
    await Task.Delay(20);
    return new Done(true);
});
app.MapPost("/echo", async (HttpRequest req) =>
{
    Echo? echo;
    try { echo = await req.ReadFromJsonAsync<Echo>(); }
    catch (JsonException) { echo = null; }
    if (echo is null) return Results.Json(new { errors = new[] { "body" } }, statusCode: 422);
    var errors = new List<string>();
    if (echo.Name is not { Length: >= 1 and <= 50 }) errors.Add("name");
    if (echo.Email?.Contains('@') != true) errors.Add("email");
    if (echo.Age is < 0 or > 150) errors.Add("age");
    if (echo.Tags is not { Length: <= 10 }) errors.Add("tags");
    return errors.Count > 0 ? Results.Json(new { errors }, statusCode: 422) : Results.Json(echo);
});
app.MapPost("/upload", async (HttpRequest req) =>
{
    long bytes = 0;
    var buffer = new byte[81920];
    int n;
    while ((n = await req.Body.ReadAsync(buffer)) > 0) bytes += n;
    return Results.Text(bytes.ToString());
});
app.MapGet("/list", () => Enumerable.Range(0, 1000).Select(i => new User(i, $"user {i}", $"user{i}@example.com", i % 3 != 0)));
app.Map("/ws", async (HttpContext ctx) =>
{
    if (!ctx.WebSockets.IsWebSocketRequest) return Results.StatusCode(400);
    using var ws = await ctx.WebSockets.AcceptWebSocketAsync();
    var buffer = new byte[4096];
    var got = await ws.ReceiveAsync(buffer, CancellationToken.None);
    while (!got.CloseStatus.HasValue)
    {
        await ws.SendAsync(buffer.AsMemory(0, got.Count), got.MessageType, got.EndOfMessage, CancellationToken.None);
        got = await ws.ReceiveAsync(buffer, CancellationToken.None);
    }
    await ws.CloseAsync(got.CloseStatus.Value, got.CloseStatusDescription, CancellationToken.None);
    return Results.Empty;
});
app.Run();

public sealed record Message(string message);

public sealed record Done(bool ok);
public sealed record Echo(string? Name, string? Email, int Age, string[]? Tags);
public sealed record User(int Id, string Name, string Email, bool Active);

// /page: 50 rows built per request, a name to escape, a class chosen by a boolean.
public sealed record Person(int Id, string Name, int Score, bool Active)
{
    static readonly string[] Names = ["Ada <&\"", "Alan <&\"", "Grace <&\"", "Linus <&\"", "Edsger <&\""];

    public static List<Person> Load() =>
        Enumerable.Range(1, 50).Select(id => new Person(id, Names[id % 5], id * 37 % 101, id % 3 != 0)).ToList();
}

public sealed record Fortune(int Id, string Message)
{
    static readonly Fortune[] Rows =
    [
        new(1, "fortune: No such file or directory"),
        new(2, "A computer scientist is someone who fixes things that aren't broken."),
        new(3, "After enough decimal places, nobody gives a damn."),
        new(4, "A bad random number generator: 1, 1, 1, 1, 1, 4.33e+67, 1, 1, 1"),
        new(5, "A computer program does what you tell it to do, not what you want it to do."),
        new(6, "Emacs is a nice operating system, but I prefer UNIX. — Tom Christaensen"),
        new(7, "Any program that runs right is obsolete."),
        new(8, "A list is only as strong as its weakest link. — Donald Knuth"),
        new(9, "Feature: A bug with seniority."),
        new(10, "Computers make very fast, very accurate mistakes."),
        new(11, "<script>alert(\"This should not be displayed in a browser alert box.\");</script>"),
        new(12, "フレームワークのベンチマーク"),
    ];

    public static List<Fortune> Load()
    {
        var list = new List<Fortune>(Rows.Length + 1);
        list.AddRange(Rows);
        list.Add(new Fortune(0, "Additional fortune added at request time."));
        list.Sort((a, b) => string.CompareOrdinal(a.Message, b.Message));
        return list;
    }
}
