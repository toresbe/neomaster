#include "ui.hpp"
#include <chrono>
#include "../modules/modules.hpp"

#include <boost/log/trivial.hpp>
#include <boost/thread/thread.hpp>

UI::UI() {
};

bool UI::register_ui_module(const UI_module &module) {
    this->modules.push_front(module);
    BOOST_LOG_TRIVIAL(info) << "UI registering module: " << module.label;
    return true;
}

void UI::start_gui() {
    this->gui = new BS5gui(this->modules);
	// If the real panel doesn't work, we just use a dummy panel that feeds various inputs.
    try {
        this->panel = new USBBS5panel();
    } catch (std::exception & e) {
		BOOST_LOG_TRIVIAL(warning) << "Could not connect to BS5 panel! Using fake panel.";
		this->panel = new FakeBS5panel();
    }
	this->panel->set_backlight(true);
	this->gui->panel = this->panel;
}

void UI::event_loop() {
    while (this->running) {
		SDL_Event event;
		if (SDL_PollEvent(&event) && event.type == SDL_QUIT)
			break;
		this->panel->read_status();
        this->gui->render();
    }
}
