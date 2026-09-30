// net/http, Gin, Fiber and bare fasthttp serving the same /fortunes,
// /plaintext, /json and /page as bench/app, all rendering with html/template
// (Gin's and Fiber's html renderers wrap it) and serializing with
// encoding/json.
//
//	bench-go nethttp|gin|fiber|fasthttp      PORT sets the port, GOMAXPROCS the threads
package main

import (
	"encoding/json"
	"html/template"
	"net/http"
	"os"
	"slices"
	"strings"

	"github.com/gin-gonic/gin"
	"github.com/gofiber/fiber/v3"
	"github.com/valyala/fasthttp"
)

type Fortune struct {
	ID      int
	Message string
}

var rows = [...]Fortune{
	{1, "fortune: No such file or directory"},
	{2, "A computer scientist is someone who fixes things that aren't broken."},
	{3, "After enough decimal places, nobody gives a damn."},
	{4, "A bad random number generator: 1, 1, 1, 1, 1, 4.33e+67, 1, 1, 1"},
	{5, "A computer program does what you tell it to do, not what you want it to do."},
	{6, "Emacs is a nice operating system, but I prefer UNIX. — Tom Christaensen"},
	{7, "Any program that runs right is obsolete."},
	{8, "A list is only as strong as its weakest link. — Donald Knuth"},
	{9, "Feature: A bug with seniority."},
	{10, "Computers make very fast, very accurate mistakes."},
	{11, "<script>alert(\"This should not be displayed in a browser alert box.\");</script>"},
	{12, "フレームワークのベンチマーク"},
}

var page = template.Must(template.New("fortunes").Parse(`<!DOCTYPE html>
<html>
<head><title>Fortunes</title></head>
<body><table>
<tr><th>id</th><th>message</th></tr>
{{range .}}
<tr><td>{{.ID}}</td><td>{{.Message}}</td></tr>
{{end}}
</table></body>
</html>
`))

// /page: a layout, a table of 50 rows built per request with a name to
// escape and a class chosen by a boolean, and a form; html/template's
// define and template are its layouts.
var roster = template.Must(template.New("layout").Parse(`<!DOCTYPE html>
<html>
<head><title>Roster</title></head>
<body>
<header><nav><a href="/">Home</a><a href="/page">Roster</a><a href="/about">About</a></nav></header>
<main>{{template "content" .}}</main>
<footer><p>Built with the framework under test.</p></footer>
</body>
</html>
{{define "content"}}
<h1>Roster</h1>
<table>
<thead><tr><th>id</th><th>name</th><th>score</th></tr></thead>
<tbody>
{{range .}}
<tr class="{{if .Active}}on{{else}}off{{end}}"><td>{{.ID}}</td><td>{{.Name}}</td><td>{{.Score}}</td></tr>
{{end}}
</tbody>
</table>
<form method="post" action="/subscribe"><label>Email <input type="email" name="email" required></label><button>Subscribe</button></form>
{{end}}`))

type Person struct {
	ID     int
	Name   string
	Score  int
	Active bool
}

var names = [...]string{`Ada <&"`, `Alan <&"`, `Grace <&"`, `Linus <&"`, `Edsger <&"`}

func people() []Person {
	list := make([]Person, 50)
	for i := range list {
		id := i + 1
		list[i] = Person{id, names[id%5], id * 37 % 101, id%3 != 0}
	}
	return list
}

type Message struct {
	Message string `json:"message"`
}

func load() []Fortune {
	list := make([]Fortune, 0, len(rows)+1)
	list = append(list, rows[:]...)
	list = append(list, Fortune{0, "Additional fortune added at request time."})
	slices.SortFunc(list, func(a, b Fortune) int { return strings.Compare(a.Message, b.Message) })
	return list
}

func main() {
	addr := "127.0.0.1:" + os.Getenv("PORT")
	switch os.Args[1] {
	case "nethttp":
		http.HandleFunc("GET /plaintext", func(w http.ResponseWriter, r *http.Request) {
			w.Header().Set("Content-Type", "text/plain; charset=utf-8")
			w.Write([]byte("Hello, World!"))
		})
		http.HandleFunc("GET /fortunes", func(w http.ResponseWriter, r *http.Request) {
			w.Header().Set("Content-Type", "text/html; charset=utf-8")
			page.Execute(w, load())
		})
		http.HandleFunc("GET /json", func(w http.ResponseWriter, r *http.Request) {
			body, _ := json.Marshal(Message{"Hello, World!"})
			w.Header().Set("Content-Type", "application/json")
			w.Write(body)
		})
		http.HandleFunc("GET /page", func(w http.ResponseWriter, r *http.Request) {
			w.Header().Set("Content-Type", "text/html; charset=utf-8")
			roster.Execute(w, people())
		})
		panic(http.ListenAndServe(addr, nil))
	case "gin":
		gin.SetMode(gin.ReleaseMode)
		app := gin.New() // gin.Default() adds a request logger
		app.GET("/plaintext", func(c *gin.Context) {
			c.String(http.StatusOK, "Hello, World!")
		})
		app.GET("/fortunes", func(c *gin.Context) {
			c.Header("Content-Type", "text/html; charset=utf-8")
			page.Execute(c.Writer, load())
		})
		app.GET("/json", func(c *gin.Context) {
			c.JSON(http.StatusOK, Message{"Hello, World!"})
		})
		app.GET("/page", func(c *gin.Context) {
			c.Header("Content-Type", "text/html; charset=utf-8")
			roster.Execute(c.Writer, people())
		})
		panic(app.Run(addr))
	case "fiber":
		app := fiber.New()
		app.Get("/plaintext", func(c fiber.Ctx) error {
			return c.SendString("Hello, World!")
		})
		app.Get("/fortunes", func(c fiber.Ctx) error {
			c.Set(fiber.HeaderContentType, fiber.MIMETextHTMLCharsetUTF8)
			return page.Execute(c.Response().BodyWriter(), load())
		})
		app.Get("/json", func(c fiber.Ctx) error {
			return c.JSON(Message{"Hello, World!"})
		})
		app.Get("/page", func(c fiber.Ctx) error {
			c.Set(fiber.HeaderContentType, fiber.MIMETextHTMLCharsetUTF8)
			return roster.Execute(c.Response().BodyWriter(), people())
		})
		panic(app.Listen(addr, fiber.ListenConfig{DisableStartupMessage: true}))
	case "fasthttp":
		// No router: a switch on the path, as its TechEmpower entry does.
		panic(fasthttp.ListenAndServe(addr, func(c *fasthttp.RequestCtx) {
			switch string(c.Path()) {
			case "/plaintext":
				c.SetContentType("text/plain; charset=utf-8")
				c.SetBodyString("Hello, World!")
			case "/fortunes":
				c.SetContentType("text/html; charset=utf-8")
				page.Execute(c, load())
			case "/json":
				body, _ := json.Marshal(Message{"Hello, World!"})
				c.SetContentType("application/json")
				c.SetBody(body)
			case "/page":
				c.SetContentType("text/html; charset=utf-8")
				roster.Execute(c, people())
			default:
				c.Error("Not Found", fasthttp.StatusNotFound)
			}
		}))
	}
}
