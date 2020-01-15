#include "spotify.hpp"
#include "spotify_glue.h"

static const char* TRANSLATE_SERVICE_NAME = "org.mpris.MediaPlayer2.spotify";
static const char* TRANSLATE_OBJECT_PATH = "/org/mpris/MediaPlayer2";

SpotifyTransport::SpotifyTransport(SpotifySource *source)
{
    printf("Starting Spotify Transport at %p (source == %p)\n", this, source);
    this->source = source;
    DBus::default_dispatcher = &dispatcher;
    auto tmp_con = DBus::Connection::SessionBus();
    bus = new DBus::Connection(tmp_con);


    dbus = new SpotifyDBus(*bus, TRANSLATE_OBJECT_PATH, TRANSLATE_SERVICE_NAME);
}

void SpotifyTransport::stop() {
    dbus->Stop();
};

void SpotifyTransport::play() {
    dbus->Play();
};

void SpotifyTransport::prev() {
    dbus->Previous();
};

void SpotifyTransport::next() {
    dbus->Next();
};

SpotifyTransport::~SpotifyTransport() {};

