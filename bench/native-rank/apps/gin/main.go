package main

import (
	"bytes"
	"html/template"
	"net/http"
	"os"

	"github.com/gin-gonic/gin"
)

type Msg struct {
	Message string `json:"message"`
}

type Row struct {
	ID     int      `json:"id"`
	Name   string   `json:"name"`
	Active bool     `json:"active"`
	Score  int      `json:"score"`
	Tags   []string `json:"tags"`
}

var listTpl = template.Must(template.New("list").Parse(
	`<!DOCTYPE html><html><head></head><body><h1>List</h1><ul>{{range .}}<li>{{.}}</li>{{end}}</ul></body></html>`))

func main() {
	gin.SetMode(gin.ReleaseMode)
	r := gin.New() // no logger, no recovery middleware: nothing per request that the others lack
	r.GET("/", func(c *gin.Context) { c.String(http.StatusOK, "Hello, World!") })
	r.GET("/json", func(c *gin.Context) { c.JSON(http.StatusOK, Msg{"Hello, World!"}) })
	r.GET("/params/:id", func(c *gin.Context) {
		sid, err := c.Cookie("sid")
		if err != nil {
			sid = "none"
		}
		c.String(http.StatusOK, "id=%s q=%s sid=%s", c.Param("id"), c.Query("q"), sid)
	})
	r.GET("/list", func(c *gin.Context) {
		items := make([]string, 1000)
		for i := range items {
			items[i] = "Item <" + itoa(i+1) + "> & co"
		}
		var b bytes.Buffer
		b.Grow(48 * 1024)
		_ = listTpl.Execute(&b, items)
		c.Data(http.StatusOK, "text/html; charset=utf-8", b.Bytes())
	})
	r.GET("/json-big", func(c *gin.Context) {
		rows := make([]Row, 200)
		for i := range rows {
			n := i + 1
			rows[i] = Row{n, "user-" + itoa(n), n%3 != 0, n * 37 % 101, []string{"a", "t" + itoa(n%7)}}
		}
		c.JSON(http.StatusOK, rows)
	})
	port := os.Getenv("PORT")
	if port == "" {
		port = "8080"
	}
	_ = r.Run("0.0.0.0:" + port)
}

func itoa(n int) string {
	var b [20]byte
	i := len(b)
	for n >= 10 {
		i--
		b[i] = byte('0' + n%10)
		n /= 10
	}
	i--
	b[i] = byte('0' + n)
	return string(b[i:])
}
