// fulmine.js, the Express API on uWebSockets.js: its the-benchmarker entry
// (javascript/fulmine.js), answering only their routes. It forks its own
// workers, WORKERS of them, each binding the port with SO_REUSEPORT.
import express from 'fulmine.js';

const app = express({ cluster: Number(process.env.WORKERS) || 'auto' });

app.set('etag', false);
app.set('connection headers', false);
// Compiles /user/:id, which only writes its parameter back, into a uWS
// declarative response answered without entering JavaScript.
app.set('declarative request values', true);

// end() sends no Content-Type, where send('') would add text/html.
app.get('/', function (req, res) {
    res.end('');
});
app.get('/user/:id', function (req, res) {
    res.end(req.params.id);
});
app.post('/user', function (req, res) {
    res.end('');
});

app.listen(Number(process.env.PORT), '127.0.0.1');
