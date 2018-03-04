#include <boost/log/trivial.hpp>
#include "ui.hpp"
#include "gui.hpp"
#include <map>
#include <string>
#define BOOST_THREAD_PROVIDES_FUTURE
#include <boost/thread.hpp>
#include <boost/thread/future.hpp>
#include <boost/log/trivial.hpp>
#include <boost/thread/thread.hpp>

#ifdef _WIN64
#include "SDL.h"
#include "SDL_ttf.h"
#else
#include "SDL2/SDL.h"
#include "SDL2/SDL_ttf.h"
#endif
namespace GUI {
	bool NeomasterUI::sdl_init()
	{
		//Initialization flag
		bool success = true;

		//initialize TTF
		if (TTF_Init() == -1) {
			printf("TTF_Init: %s\n", TTF_GetError());
			exit(2);
		}

		//Initialize SDL
		if (SDL_Init(SDL_INIT_VIDEO) < 0)
		{
			printf("SDL could not initialize! SDL_Error: %s\n", SDL_GetError());
			success = false;
		}
		else
		{
			//Create window
			this->window = SDL_CreateWindow("neomedia", SDL_WINDOWPOS_UNDEFINED, SDL_WINDOWPOS_UNDEFINED, SCREEN_WIDTH, SCREEN_HEIGHT, SDL_WINDOW_SHOWN);
			if (this->window == NULL)
			{
				printf("Window could not be created! SDL_Error: %s\n", SDL_GetError());
				success = false;
			}
		}

		this->renderer = SDL_CreateRenderer(this->window, -1, 0);

		return success;
	}

	NeomasterUI::NeomasterUI(Modules::module_list_t & module_list) {
		sdl_init();
		for (auto module : module_list) {
			if (module->get_ui_module() != nullptr) {
				module->get_ui_module()->attach_ui(this);
				this->ui_module_list.push_front(module->get_ui_module());
			}
		}
		this->module_wheel = new GUI::ModuleWheel(this->renderer, this->ui_module_list);

		draw();
	}

	void NeomasterUI::handle_panel_input(const Panel::input_damage_t & damage, const Panel::input_state_t & state) {
		if (damage.wheel_3) {
			this->module_wheel->handle_panel_input(damage, state);
		}
		if (this->module_wheel->selected_module != nullptr) {
			this->module_wheel->selected_module->handle_panel_input(damage, state);
		}
		this->draw();
	}

	void NeomasterUI::draw() {
		if (this->module_wheel->selected_module != nullptr) {
			this->module_wheel->selected_module->draw();
		}
		else {
			SDL_SetRenderDrawColor(this->renderer, 0, 0, 0, 255);
			SDL_RenderClear(this->renderer);
		}
		this->module_wheel->render();
		SDL_RenderPresent(this->renderer);
	}


	void NeomasterUI::start_gui() {
		// If the real panel doesn't work, we just use a dummy panel that feeds various inputs.
		try {
			this->panel = new Panel::USBDevice();
		}
		catch (std::exception & e) {
			BOOST_LOG_TRIVIAL(warning) << "Could not connect to BS5 panel! Using fake panel.";
			this->panel = new Panel::FakeDevice();
		}
		this->panel->set_backlight(true);
	}

	void NeomasterUI::event_loop() {
		this->draw();
		Panel::input_damage_t damage;
		while (this->running) {
			boost::packaged_task<Panel::input_damage_t> pt(std::bind(&Panel::Device::read_status, this->panel));
			boost::future<Panel::input_damage_t> fi1 = pt.get_future();
			boost::thread task(boost::move(pt));

			SDL_Event event;
			if (SDL_PollEvent(&event) && event.type == SDL_QUIT)
				break;

			boost::wait_for_any(fi1);
			if (fi1.is_ready()) {
				damage = fi1.get();
				if (this->panel->input.bs5_state.button_ok) running = false;
				handle_panel_input(damage, this->panel->input.bs5_state);
			}
		}
	}
}