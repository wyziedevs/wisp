#[derive(Rest)]                    // Json + FromJson + Note::table()
#[rest(write = "API_KEY")]         // writes need Bearer $API_KEY; key = all, admin = DELETE
struct Note {
    #[validate(len = 1..=200)]     // len, min, max, min_len, max_len, email
    title: String,
    done: bool,                    // left out: false; Vec: []; Option: None
    tags: Vec<String>,
    created_at: String,            // set by Wisp (RFC 3339; u64 = unix seconds); also updated_at
}
fn before_create(note: &mut Note) -> Result { Ok(()) }  // also before_update(id, note),
fn after_update(note: &Row<Note>) {}                     // before_delete/after_*(row); cx optional
