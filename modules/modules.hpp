#ifndef MODULES_HPP

#define MODULES_HPP

#include <forward_list>
#include <string>
#include "ui/panel.hpp"


#ifdef _WIN64
#include "SDL.h"
#include "SDL_ttf.h"
#else
#include "SDL2/SDL.h"
#include "SDL2/SDL_ttf.h"
#endif

class NeomasterUI;
namespace Modules {
	class NeomasterModuleUI {
	public:
		virtual void show() = 0;
		virtual void initialize() = 0;
		virtual void draw() = 0;
		virtual void handle_panel_input(const Panel::input_damage_t & damage, const Panel::input_state_t & state) = 0;
		void attach_ui(NeomasterUI * ui);
		std::string label;
		std::string description;
		NeomasterUI * ui;
	protected:
	};

	typedef std::forward_list<NeomasterModuleUI *> ui_module_list_t;

	class NeomasterModule {
	public:
		virtual NeomasterModuleUI *get_ui_module() = 0;
	};

	typedef std::forward_list<NeomasterModule *> module_list_t;

	class Manager {
	public:
		Manager();
		module_list_t module_list;
	};


}
#endif