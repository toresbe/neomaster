#pragma once
#include "beosource.pb.h"

class IEventHandler {
    public:
        virtual beosource::Response handle(const beosource::Event & event) = 0;
};

class ITransport {
    public:
        virtual ~ITransport() = default;
        virtual void play() = 0;
        virtual void stop() = 0;
        virtual void prev() = 0;
        virtual void next() = 0;
};

class INeomaster {
    public:
        virtual ~INeomaster() = default;
        uint8_t active_source = 0;
};

class ISource {
    public:
        INeomaster *kernel;
        // Masterlink source ID number
        uint8_t masterlink_id = 0x00;
        IEventHandler *events;
        ITransport *transport;
};
