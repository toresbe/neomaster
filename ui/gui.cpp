#include <boost/log/trivial.hpp>
#include "ui.hpp"
#include "gui.hpp"
#include <map>
#include <string>

#ifdef _WIN64
#include "SDL.h"
#include "SDL_ttf.h"
#else
#include "SDL2/SDL.h"
#include "SDL2/SDL_ttf.h"
#endif

bool NeomasterUI::sdl_init()
{
    //Initialization flag
    bool success = true;

    //initialize TTF
    if(TTF_Init()==-1) {
        printf("TTF_Init: %s\n", TTF_GetError());
        exit(2);
    }

    //Initialize SDL
    if( SDL_Init( SDL_INIT_VIDEO ) < 0 )
    {
        printf( "SDL could not initialize! SDL_Error: %s\n", SDL_GetError() );
        success = false;
    }
    else
    {
        //Create window
        this->window = SDL_CreateWindow( "neomedia", SDL_WINDOWPOS_UNDEFINED, SDL_WINDOWPOS_UNDEFINED, SCREEN_WIDTH, SCREEN_HEIGHT, SDL_WINDOW_SHOWN);
        if( this->window == NULL )
        {
            printf( "Window could not be created! SDL_Error: %s\n", SDL_GetError() );
            success = false;
        }
        else
        {
            //Get window surface
            this->window_surface = SDL_GetWindowSurface( this->window );
        }
    }

	this->renderer = SDL_CreateRenderer(this->window, -1, 0);

    return success;
}

NeomasterUI::NeomasterUI(module_list_t & module_list) {
    sdl_init();
	for (auto module: module_list) {
		if (module->get_ui_module() != nullptr) {
			module->get_ui_module()->attach_ui(this);
			this->ui_module_list.push_front(module->get_ui_module());
		}
	}
    this->module_wheel = new ModuleWheel(this->renderer, this->ui_module_list);
	this->active_module = *this->ui_module_list.begin();
	this->active_module->show();
    draw();
}

void NeomasterUI::handle_panel_input(const BS5input::bs5_damage_t & damage, const BS5input::bs5_state_t & state) {
	if (damage.wheel_3) {
		this->module_wheel->handle_panel_input(damage, state);
		//this->module_wheel
	}
	this->draw();
}

void NeomasterUI::draw() {
	this->active_module->draw();
	this->module_wheel->render();
	SDL_RenderPresent(this->renderer);
}


void NeomasterUI::add_widget(NMWidget * widget) {
	this->widget_list.push_back(widget);
}

/*void NeomasterUI::draw() {
	for (NMWidget *widget : widget_list) {
		widget->draw();
	}
}*/