#include "spotify.hpp"
#include "spotify_glue.h"

static const char* TRANSLATE_SERVICE_NAME = "org.mpris.MediaPlayer2.spotify";
static const char* TRANSLATE_OBJECT_PATH = "/org/mpris/MediaPlayer2";

SpotifyTransport::SpotifyTransport(ISource *source) {
    this->source = source;
    DBus::BusDispatcher dispatcher;
    DBus::default_dispatcher = &dispatcher;
    DBus::Connection bus = DBus::Connection::SessionBus();

    dbus = new SpotifyDBus(bus, TRANSLATE_OBJECT_PATH, TRANSLATE_SERVICE_NAME);
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

