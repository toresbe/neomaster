#include <boost/log/trivial.hpp>
#include <google/protobuf/descriptor.h>
#include "../sources.hpp"
#include "spotify.hpp"

SpotifyEventHandler::SpotifyEventHandler(SpotifySource *source) {
    this->source = source;
};

std::string ml_source_name(int src_id) {
    const google::protobuf::EnumDescriptor* enum_desc =
        beosource::Source::MLSource_descriptor();

    return enum_desc->FindValueByNumber(src_id)->name();
}

beosource::Response SpotifyEventHandler::handle_source_change(const beosource::Event & event) {
    beosource::Response response;

    auto new_source_name = ml_source_name(event.source_change().new_source());
    auto old_source_name = ml_source_name(event.source_change().old_source());

    // Are we the source being switched away from?
    if( (event.source_change().new_source() != source->masterlink_id) &&
            (event.source_change().old_source() == source->masterlink_id) 
      ) 
    {
        BOOST_LOG_TRIVIAL(debug) << "Halting source for source change from " << \
            old_source_name << " to " << new_source_name;

        this->source->transport->stop();
        response.set_code(beosource::Response::ACK);
    } else if (event.source_change().new_source() == source->masterlink_id) {
        BOOST_LOG_TRIVIAL(debug) << "Starting source " << new_source_name << \
            " (source was " << old_source_name << ")";
        this->source->transport->play();
        response.set_code(beosource::Response::ACK);
    } else {
        response.set_code(beosource::Response::NOP);
    }
    return response;
}

beosource::Response SpotifyEventHandler::handle_keypress(const beosource::Event & event) {
    beosource::Response response;
    bool key_handled = true;
    switch(event.key_press().keycode()) {
        case beosource::KeyPress::stop:
            this->source->transport->stop();
            break;
        case beosource::KeyPress::go:
            this->source->transport->play();
            break;
        case beosource::KeyPress::arrow_right:
            this->source->transport->next();
            break;
        case beosource::KeyPress::arrow_left:
            this->source->transport->prev();
            break;
        default:
            key_handled = false;
    }

    if(key_handled) {
        response.set_code(beosource::Response::ACK);
    } else {
        response.set_code(beosource::Response::NOP);
    }
    return response;
}

beosource::Response SpotifyEventHandler::handle(const beosource::Event & event) {
    beosource::Response response;
    if(event.has_key_press()) {
        return handle_keypress(event);
    } else if(event.has_source_change()) {
        return handle_source_change(event);
    } else {
        response.set_code(beosource::Response::NAK);
    }
    return response;
};
