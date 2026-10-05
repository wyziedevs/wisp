package nr;

import java.util.ArrayList;
import java.util.List;
import org.springframework.boot.SpringApplication;
import org.springframework.boot.autoconfigure.SpringBootApplication;
import org.springframework.web.bind.annotation.CookieValue;
import org.springframework.web.bind.annotation.GetMapping;
import org.springframework.web.bind.annotation.PathVariable;
import org.springframework.web.bind.annotation.RequestParam;
import org.springframework.web.bind.annotation.RestController;
import org.springframework.web.util.HtmlUtils;

@SpringBootApplication
@RestController
public class App {
    record Msg(String message) {}
    record Row(int id, String name, boolean active, int score, List<String> tags) {}

    public static void main(String[] args) {
        SpringApplication.run(App.class, args);
    }

    @GetMapping(value = "/", produces = "text/plain;charset=UTF-8")
    String index() {
        return "Hello, World!";
    }

    @GetMapping("/json")
    Msg json() {
        return new Msg("Hello, World!");
    }

    @GetMapping(value = "/params/{id}", produces = "text/plain;charset=UTF-8")
    String params(@PathVariable String id, @RequestParam(defaultValue = "") String q, @CookieValue(name = "sid", defaultValue = "none") String sid) {
        return "id=" + id + " q=" + q + " sid=" + sid;
    }

    @GetMapping(value = "/list", produces = "text/html;charset=UTF-8")
    String list() {
        StringBuilder sb = new StringBuilder(48 * 1024);
        sb.append("<!DOCTYPE html><html><head></head><body><h1>List</h1><ul>");
        for (int i = 1; i <= 1000; i++) sb.append("<li>").append(HtmlUtils.htmlEscape("Item <" + i + "> & co")).append("</li>");
        return sb.append("</ul></body></html>").toString();
    }

    @GetMapping("/json-big")
    List<Row> jsonBig() {
        List<Row> rows = new ArrayList<>(200);
        for (int i = 1; i <= 200; i++) rows.add(new Row(i, "user-" + i, i % 3 != 0, i * 37 % 101, List.of("a", "t" + i % 7)));
        return rows;
    }
}
