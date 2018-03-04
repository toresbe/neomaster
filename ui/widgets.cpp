#include "ui/widgets.hpp"
#ifdef _WIN64
#include "SDL.h"
#include "SDL_ttf.h"
#else
#include "SDL2/SDL.h"
#include "SDL2/SDL_ttf.h"
#endif

namespace Widget {
	NMProgressBar::NMProgressBar(GUI::NeomasterUI * ui, int x, int y, int w, int h) {
		this->x = x;
		this->y = y;
		this->w = w;
		this->h = h;
		this->frame = { x, y, w, h };
		this->ui = ui;
		set_value(0);
	}

	inline void NMProgressBar::set_value(float value) {
		this->bar = { this->x + 1, this->y + 1, (int)(this->w * value) - 2, this->h - 2 };
		this->gap = { this->x + bar.w, this->y, (int)(this->w * value), this->h };
	}

	inline void NMProgressBar::draw() {
		SDL_SetRenderDrawColor(this->ui->renderer, 255, 255, 255, 255);
		//SDL_RenderFillRect(this->ui->renderer, &this->gap);
		SDL_RenderFillRect(this->ui->renderer, &this->frame);

		SDL_SetRenderDrawColor(this->ui->renderer, 0, 0, 0, 255);
		SDL_RenderFillRect(this->ui->renderer, &this->bar);
	}
};