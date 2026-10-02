// net/http, Gin, Fiber and bare fasthttp serving the same /fortunes,
// /plaintext, /json and /page as bench/app, all rendering with html/template
// (Gin's and Fiber's html renderers wrap it) and serializing with
// encoding/json; and the-benchmarker's GET /, GET /user/:id and POST /user
// as each one's entry there answers them.
//
//	bench-go nethttp|gin|fiber|fasthttp      PORT sets the port, GOMAXPROCS the threads
package main

import (
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"html/template"
	"io"
	"net/http"
	"os"
	"slices"
	"strconv"
	"strings"
	"time"
	"unicode/utf8"

	"github.com/coder/websocket"
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

// The practice routes (bench/README.md), net/http only.
type Echo struct {
	Name  string   `json:"name"`
	Email string   `json:"email"`
	Age   int      `json:"age"`
	Tags  []string `json:"tags"`
}

type User struct {
	ID     int    `json:"id"`
	Name   string `json:"name"`
	Email  string `json:"email"`
	Active bool   `json:"active"`
}

func writeJSON(w http.ResponseWriter, status int, v any) {
	body, _ := json.Marshal(v)
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(status)
	w.Write(body)
}

func echo(w http.ResponseWriter, r *http.Request) {
	var e Echo
	if err := json.NewDecoder(r.Body).Decode(&e); err != nil {
		writeJSON(w, 422, map[string][]string{"errors": {"body"}})
		return
	}
	errs := []string{}
	if n := utf8.RuneCountInString(e.Name); n < 1 || n > 50 {
		errs = append(errs, "name")
	}
	if !strings.Contains(e.Email, "@") {
		errs = append(errs, "email")
	}
	if e.Age < 0 || e.Age > 150 {
		errs = append(errs, "age")
	}
	if len(e.Tags) > 10 {
		errs = append(errs, "tags")
	}
	if len(errs) > 0 {
		writeJSON(w, 422, map[string][]string{"errors": errs})
		return
	}
	if e.Tags == nil {
		e.Tags = []string{}
	}
	writeJSON(w, 200, e)
}

func upload(w http.ResponseWriter, r *http.Request) {
	body, err := io.ReadAll(http.MaxBytesReader(w, r.Body, 8<<20))
	var tooBig *http.MaxBytesError
	if errors.As(err, &tooBig) {
		http.Error(w, "Payload Too Large", http.StatusRequestEntityTooLarge)
		return
	}
	w.Header().Set("Content-Type", "text/plain; charset=utf-8")
	w.Write([]byte(strconv.Itoa(len(body))))
}

func list(w http.ResponseWriter, r *http.Request) {
	users := make([]User, 1000)
	for i := range users {
		users[i] = User{i, fmt.Sprintf("user %d", i), fmt.Sprintf("user%d@example.com", i), i%3 != 0}
	}
	writeJSON(w, 200, users)
}

func echoSocket(w http.ResponseWriter, r *http.Request) {
	c, err := websocket.Accept(w, r, nil)
	if err != nil {
		return
	}
	defer c.CloseNow()
	for {
		kind, msg, err := c.Read(r.Context())
		if err != nil || c.Write(r.Context(), kind, msg) != nil {
			return
		}
	}
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
		http.HandleFunc("GET /", func(w http.ResponseWriter, r *http.Request) {
			w.Write([]byte(""))
		})
		http.HandleFunc("GET /user/{name}", func(w http.ResponseWriter, r *http.Request) {
			w.Write([]byte(r.PathValue("name")))
		})
		http.HandleFunc("POST /user", func(w http.ResponseWriter, r *http.Request) {
			w.Write([]byte(""))
		})
		http.HandleFunc("GET /wait", func(w http.ResponseWriter, r *http.Request) {
			time.Sleep(20 * time.Millisecond)
			writeJSON(w, 200, map[string]bool{"ok": true})
		})
		http.HandleFunc("POST /echo", echo)
		http.HandleFunc("POST /upload", upload)
		http.HandleFunc("GET /list", list)
		http.Handle("GET /static/", http.StripPrefix("/static/", http.FileServer(http.Dir("../static"))))
		http.HandleFunc("GET /ws", echoSocket)
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
		app.GET("/", func(c *gin.Context) {
			c.Writer.Write([]byte(""))
		})
		app.GET("/user/:name", func(c *gin.Context) {
			c.Writer.Write([]byte(c.Params.ByName("name")))
		})
		app.POST("/user", func(c *gin.Context) {
			c.Writer.Write([]byte(""))
		})
		panic(app.Run(addr))
	case "fiber":
		// As Fiber's the-benchmarker entry configures it.
		app := fiber.New(fiber.Config{
			CaseSensitive:            true,
			StrictRouting:            true,
			DisableHeaderNormalizing: true,
		})
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
		ok := func(c fiber.Ctx) error { return nil }
		app.Get("/", ok)
		app.Get("/user/:id", func(c fiber.Ctx) error {
			return c.SendString(c.Params("id"))
		})
		app.Post("/user", ok)
		panic(app.Listen(addr, fiber.ListenConfig{DisableStartupMessage: true}))
	case "fasthttp":
		// No router: a switch on the path, as its TechEmpower entry does.
		panic(fasthttp.ListenAndServe(addr, func(c *fasthttp.RequestCtx) {
			path := c.Path()
			switch string(path) {
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
			case "/", "/user":
			default:
				if id, ok := bytes.CutPrefix(path, []byte("/user/")); ok {
					c.SetBody(id)
				} else {
					c.Error("Not Found", fasthttp.StatusNotFound)
				}
			}
		}))
	}
}
