#ifndef GUI_HPP

#define GUI_HPP

#define SCREEN_WIDTH 1024
#define SCREEN_HEIGHT 768

#ifdef _WIN64
#include "SDL.h"
#include "SDL_ttf.h"
#else
#include "SDL2/SDL.h"
#include "SDL2/SDL_ttf.h"
#endif

#include "../modules/modules.hpp"
#include "ui/widgets.hpp"
#include "panel.hpp"
#include "module_wheel.hpp"

class NMWidget;
class NeomasterUI {
public:
	NeomasterUI(module_list_t & module_list);
	void handle_panel_input(const BS5input::bs5_damage_t & damage, const BS5input::bs5_state_t & state);
	BS5panel *panel = nullptr;
	void add_widget(NMWidget *widget);
    void draw();
    SDL_Renderer * renderer;
    typedef std::list<NMWidget *> widget_list_t;
	ui_module_list_t ui_module_list;
    widget_list_t	widget_list;
private:
	NeomasterModuleUI * active_module;
	ModuleWheel* module_wheel;
	bool sdl_init();
	SDL_Window* window;
	SDL_Surface* window_surface;
	std::forward_list<SDL_Surface **> layer_list;
};

#endif
