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

class NeomasterModuleUI {
public:
	virtual void show() = 0;
	virtual void initialize() = 0;
	virtual void draw() = 0;
	virtual void handle_panel_input(const BS5input::bs5_damage_t & damage, const BS5input::bs5_state_t & state) = 0;
	void attach_ui(NeomasterUI * ui);
	std::string label;
	std::string description;
	NeomasterUI * ui;
protected:
	/*
	
	virtual void moduleDeselected() = 0;
	virtual void panel_event() = 0;
	*/
};

typedef std::forward_list<NeomasterModuleUI *> ui_module_list_t;

class NeomasterModule {
    public:
		virtual NeomasterModuleUI *get_ui_module() = 0;
};

typedef std::forward_list<NeomasterModule *> module_list_t;

class ModuleManager {
    public:
        ModuleManager();
        module_list_t module_list;
};


#endif