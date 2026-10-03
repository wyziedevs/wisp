#[derive(Rest)]                    // Json + FromJson + Note::table()
#[rest(write = "API_KEY")]         // writes need Bearer $API_KEY
struct Note {
    #[validate(len = 1..=200)]
    title: String,
    done: bool,                    // left out: false; Vec: []; Option: None
    created_at: String,            // set by Wisp (also updated_at)
}
fn before_create(note: &mut Note) -> Result { Ok(()) }  // also before_update, after_*
