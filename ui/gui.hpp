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
namespace Widget { class NMWidget; };
typedef std::list<Widget::NMWidget *> widget_list_t;

class NeomasterUI {
public:
	void event_loop();
	bool register_module(Modules::NeomasterModule *module);
	void start_gui();
	NeomasterUI(Modules::module_list_t & module_list);
	void handle_panel_input(const Panel::input_damage_t & damage, const Panel::input_state_t & state);
	Panel::Device *panel = nullptr;
    void draw();
    SDL_Renderer * renderer;
	Modules::ui_module_list_t ui_module_list;
    widget_list_t	widget_list;
private:
	Modules::NeomasterModuleUI * active_module;
	GUI::ModuleWheel* module_wheel;
	bool sdl_init();
	SDL_Window* window;
	SDL_Surface* window_surface;
	std::forward_list<SDL_Surface **> layer_list;
	Modules::module_list_t modules{};
	bool running = true;
};

#endif
