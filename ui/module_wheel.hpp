#ifndef MODULE_WHEEL_HPP

#define MODULE_WHEEL_HPP

#ifdef _WIN64
#include "SDL.h"
#include "SDL_ttf.h"
#else
#include "SDL2/SDL.h"
#include "SDL2/SDL_ttf.h"
#endif

#include "modules/modules.hpp"

namespace GUI {
	class ModuleWheel {
	public:
		void module_wheel_moved();
		Modules::NeomasterModuleUI * new_selected_module(int posWheel3);
		void render_background();
		ModuleWheel(SDL_Renderer * renderer, Modules::ui_module_list_t module_list);
		void render();
		void handle_panel_input(const Panel::input_damage_t & damage, const Panel::input_state_t & state);
		void add_module_bitmap_entry(Modules::NeomasterModuleUI * module, const SDL_Rect & rect);
		Modules::NeomasterModuleUI * selected_module;
		SDL_Color fg_color = { 255, 255, 255, 255 };
		SDL_Color bg_color = { 255, 255, 255, 255 };
	private:
		typedef struct module_bitmap_entry_t {
			int top;
			int bottom;
			Modules::NeomasterModuleUI *module;
		} module_bitmap_entry_t;
		std::forward_list<module_bitmap_entry_t> module_bitmap_list;

		Panel::input_state_t input_state;
		SDL_Renderer * renderer;
		void render_text(const std::string & text, SDL_Rect rect);
		void render_modules();
		SDL_Rect get_rekt(int num_modules, int module_index);
		void render_beam();
		Modules::ui_module_list_t module_list;
		TTF_Font *font;
		int font_line_height;
	};
}
#endif
