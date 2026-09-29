// net/http and Fiber serving the same /fortunes and /plaintext as
// bench/app, both rendering with html/template (Fiber's html view engine
// is a wrapper around it).
//
//	bench-go nethttp|fiber      PORT sets the port, GOMAXPROCS the threads
package main

import (
	"html/template"
	"net/http"
	"os"
	"slices"
	"strings"

	"github.com/gofiber/fiber/v3"
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
		panic(http.ListenAndServe(addr, nil))
	case "fiber":
		app := fiber.New()
		app.Get("/plaintext", func(c fiber.Ctx) error {
			return c.SendString("Hello, World!")
		})
		app.Get("/fortunes", func(c fiber.Ctx) error {
			c.Set(fiber.HeaderContentType, fiber.MIMETextHTMLCharsetUTF8)
			return page.Execute(c.Response().BodyWriter(), load())
		})
		panic(app.Listen(addr, fiber.ListenConfig{DisableStartupMessage: true}))
	}
}
