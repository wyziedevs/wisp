using System.Text.Encodings.Web;
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

var app = builder.Build();
app.MapGet("/plaintext", () => "Hello, World!");
app.MapGet("/json", () => new Message("Hello, World!"));
app.MapRazorPages(); // /fortunes and /page
app.MapGet("/fortunes-blazor", () => new RazorComponentResult<FortunesPage>(new { Fortunes = Fortune.Load() }));
app.Run();

public sealed record Message(string message);

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
