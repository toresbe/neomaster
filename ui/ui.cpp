#include "ui.hpp"
#include <functional>
#include <chrono>
#include "../modules/modules.hpp"
#define BOOST_THREAD_PROVIDES_FUTURE
#include <boost/thread.hpp>
#include <boost/thread/future.hpp>
#include <boost/log/trivial.hpp>
#include <boost/thread/thread.hpp>


bool UI::register_module(NeomasterModule *module) {
    this->modules.push_front(module);
    BOOST_LOG_TRIVIAL(info) << "UI registering module: " << module->get_ui_module()->label;
    return true;
}

void UI::start_gui() {
    this->gui = new NeomasterUI(this->modules);
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
	this->gui->draw();
	BS5input::bs5_damage_t damage;
    while (this->running) {
		boost::packaged_task<BS5input::bs5_damage_t> pt(std::bind(&BS5panel::read_status, this->panel));
		boost::future<BS5input::bs5_damage_t> fi1 = pt.get_future();
		boost::thread task(boost::move(pt));
		
		SDL_Event event;
		if (SDL_PollEvent(&event) && event.type == SDL_QUIT)
			break;
		
		boost::wait_for_any(fi1);
		if (fi1.is_ready()) {
			damage = fi1.get();
			this->gui->handle_panel_input(damage, this->panel->input.bs5_state);
		}
    }
}