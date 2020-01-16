#include <GUnit.h>
#include "GUnit/GMock.h"
#include "gmock/gmock.h"
#include "spotify.hpp"

using ::testing::StrictGMock;

GTEST(SpotifyEventHandler) {
    SpotifySource server((INeomaster *)nullptr);
    server.masterlink_id = beosource::Source::a_mem;
    StrictGMock<INeomaster> mock_kernel;
    StrictGMock<SpotifyTransport> mock_transport;
    server.transport = object(mock_transport);
    server.kernel = object(mock_kernel);
    server.events = new SpotifyEventHandler(&server);

    beosource::Event event;
    beosource::Response response;

    event.mutable_bogus_event();

    SHOULD("Respond NAK on invalid command") {
        response = server.events->handle(event);
        EXPECT_EQ(response.code(), beosource::Response::NAK);
    }

    // Side effect: Turns event into key press event
    auto key_press = event.mutable_key_press();

    SHOULD("Respond NOP on unhandled keypress") {
        // vol up is handled by underlying layers and should
        // not even be forwarded to the source in the first 
        // place
        key_press->set_keycode(beosource::KeyPress::vol_up);
        response = server.events->handle(event);
        EXPECT_EQ(response.code(), beosource::Response::NOP);
    }

    SHOULD("Issue prev track to transport when arrow left is received") {
        EXPECT_INVOKE(mock_transport, prev);
        key_press->set_keycode(beosource::KeyPress::arrow_left);
        response = server.events->handle(event);
        EXPECT_EQ(response.code(), beosource::Response::ACK);
    } 

    SHOULD("Issue next track to transport when arrow right is received") {
        EXPECT_INVOKE(mock_transport, next);
        key_press->set_keycode(beosource::KeyPress::arrow_right);
        response = server.events->handle(event);
        EXPECT_EQ(response.code(), beosource::Response::ACK);
    } 

    SHOULD("Issue play command to transport when go code is received") {
        EXPECT_INVOKE(mock_transport, play);
        key_press->set_keycode(beosource::KeyPress::go);
        response = server.events->handle(event);
        EXPECT_EQ(response.code(), beosource::Response::ACK);
    } 

    SHOULD("Issue stop command to transport when stop code is received") {
        EXPECT_INVOKE(mock_transport, stop);
        key_press->set_keycode(beosource::KeyPress::stop);
        response = server.events->handle(event);
        EXPECT_EQ(response.code(), beosource::Response::ACK);
    } 

    auto source_change = event.mutable_source_change();

    SHOULD("Return NOP when switching between other sources") {
        source_change->set_old_source(beosource::Source::radio);
        source_change->set_new_source(beosource::Source::cd);

        response = server.events->handle(event);
        EXPECT_EQ(response.code(), beosource::Response::NOP);
    } 

    SHOULD("Issue stop command to transport when other source selected") {
        source_change->set_old_source(beosource::Source::a_mem);
        source_change->set_new_source(beosource::Source::radio);

        EXPECT_INVOKE(mock_transport, stop);
        response = server.events->handle(event);
        EXPECT_EQ(response.code(), beosource::Response::ACK);
    } 

    SHOULD("Issue play command to transport when source is selected anyway") {
        source_change->set_old_source(beosource::Source::a_mem);
        source_change->set_new_source(beosource::Source::a_mem);

        EXPECT_INVOKE(mock_transport, play);
        response = server.events->handle(event);
        EXPECT_EQ(response.code(), beosource::Response::ACK);
    }

    SHOULD("Issue play command to transport when going from other src. to this") {
        source_change->set_old_source(beosource::Source::radio);
        source_change->set_new_source(beosource::Source::a_mem);

        EXPECT_INVOKE(mock_transport, play);
        response = server.events->handle(event);
        EXPECT_EQ(response.code(), beosource::Response::ACK);
    }
}
