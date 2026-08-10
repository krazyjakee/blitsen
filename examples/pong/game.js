const game = document.getElementById("game");
const court = document.getElementById("court");
const leftPaddle = document.getElementById("left-paddle");
const rightPaddle = document.getElementById("right-paddle");
const ball = document.getElementById("ball");
const leftScore = document.getElementById("left-score");
const rightScore = document.getElementById("right-score");
const messageTitle = document.getElementById("message-title");
const messageDetail = document.getElementById("message-detail");
const fps = document.getElementById("fps");

const WIDTH = 640;
const HEIGHT = 340;
const PADDLE_HEIGHT = 80;
const PADDLE_SPEED = 430;
const BALL_SIZE = 22;
const LEFT_X = 28;
const RIGHT_X = 594;
const WINNING_SCORE = 7;

const keys = new Set();
const state = {
  playing: false,
  winner: false,
  leftY: (HEIGHT - PADDLE_HEIGHT) / 2,
  rightY: (HEIGHT - PADDLE_HEIGHT) / 2,
  ballX: (WIDTH - BALL_SIZE) / 2,
  ballY: (HEIGHT - BALL_SIZE) / 2,
  ballVX: 330,
  ballVY: 145,
  leftScore: 0,
  rightScore: 0,
  previousTime: null,
  frameCount: 0,
  sampleStarted: null,
};

const clamp = (value, minimum, maximum) => Math.max(minimum, Math.min(maximum, value));

function setMessage(title, detail) {
  messageTitle.textContent = title;
  messageDetail.textContent = detail;
}

function setMode(mode) {
  game.classList.remove("playing", "paused", "winner");
  game.classList.add(mode);
  game.setAttribute("data-state", mode);
}

function resetBall(direction) {
  state.ballX = (WIDTH - BALL_SIZE) / 2;
  state.ballY = (HEIGHT - BALL_SIZE) / 2;
  state.ballVX = 330 * direction;
  state.ballVY = (state.leftScore + state.rightScore) % 2 === 0 ? 145 : -145;
}

function startRound() {
  if (state.winner) {
    state.leftScore = 0;
    state.rightScore = 0;
    leftScore.textContent = "0";
    rightScore.textContent = "0";
    state.winner = false;
    resetBall(1);
  }
  state.playing = true;
  state.previousTime = null;
  setMode("playing");
  court.focus();
}

function togglePlay() {
  if (!state.playing) startRound();
  else {
    state.playing = false;
    setMode("paused");
    setMessage("PAUSED", "Press Space to resume");
  }
}

function score(side) {
  state.playing = false;
  if (side === "left") {
    state.leftScore += 1;
    leftScore.textContent = String(state.leftScore);
  } else {
    state.rightScore += 1;
    rightScore.textContent = String(state.rightScore);
  }
  const scoreValue = side === "left" ? state.leftScore : state.rightScore;
  if (scoreValue === WINNING_SCORE) {
    state.winner = true;
    setMode("winner");
    setMessage(side === "left" ? "PLAYER ONE WINS" : "PLAYER TWO WINS", "Press Space for a new match");
  } else {
    resetBall(side === "left" ? -1 : 1);
    setMode("paused");
    setMessage(`${side === "left" ? "PLAYER ONE" : "PLAYER TWO"} SCORES`, "Press Space to serve");
  }
}

function paddleCollision(paddleX, paddleY, movingRight) {
  const crossedX = movingRight
    ? state.ballX + BALL_SIZE >= paddleX && state.ballX + BALL_SIZE <= paddleX + 18
    : state.ballX <= paddleX + 14 && state.ballX >= paddleX - 4;
  if (!crossedX || state.ballY + BALL_SIZE < paddleY || state.ballY > paddleY + PADDLE_HEIGHT) return;
  const impact = ((state.ballY + BALL_SIZE / 2) - (paddleY + PADDLE_HEIGHT / 2)) / (PADDLE_HEIGHT / 2);
  const speed = Math.min(610, Math.abs(state.ballVX) + 22);
  state.ballVX = speed * (movingRight ? -1 : 1);
  state.ballVY = impact * 360;
  state.ballX = movingRight ? paddleX - BALL_SIZE : paddleX + 14;
}

function update(seconds) {
  const leftDirection = (keys.has("w") ? -1 : 0) + (keys.has("s") ? 1 : 0);
  const rightDirection = (keys.has("arrowup") ? -1 : 0) + (keys.has("arrowdown") ? 1 : 0);
  state.leftY = clamp(state.leftY + leftDirection * PADDLE_SPEED * seconds, 0, HEIGHT - PADDLE_HEIGHT);
  state.rightY = clamp(state.rightY + rightDirection * PADDLE_SPEED * seconds, 0, HEIGHT - PADDLE_HEIGHT);
  if (!state.playing) return;

  state.ballX += state.ballVX * seconds;
  state.ballY += state.ballVY * seconds;
  if (state.ballY <= 0) {
    state.ballY = 0;
    state.ballVY = Math.abs(state.ballVY);
  } else if (state.ballY >= HEIGHT - BALL_SIZE) {
    state.ballY = HEIGHT - BALL_SIZE;
    state.ballVY = -Math.abs(state.ballVY);
  }
  if (state.ballVX > 0) paddleCollision(RIGHT_X, state.rightY, true);
  else paddleCollision(LEFT_X, state.leftY, false);
  if (state.ballX < -BALL_SIZE) score("right");
  else if (state.ballX > WIDTH) score("left");
}

function render() {
  leftPaddle.style.top = `${state.leftY}px`;
  rightPaddle.style.top = `${state.rightY}px`;
  ball.style.left = `${state.ballX}px`;
  ball.style.top = `${state.ballY}px`;
}

function frame(timestamp) {
  if (state.sampleStarted === null) state.sampleStarted = timestamp;
  const elapsed = state.previousTime === null ? 0 : Math.min(0.034, (timestamp - state.previousTime) / 1000);
  state.previousTime = timestamp;
  update(elapsed);
  render();
  state.frameCount += 1;
  const sampleTime = timestamp - state.sampleStarted;
  if (sampleTime >= 500) {
    fps.textContent = String(Math.round(state.frameCount * 1000 / sampleTime));
    state.frameCount = 0;
    state.sampleStarted = timestamp;
  }
  requestAnimationFrame(frame);
}

window.addEventListener("keydown", event => {
  const key = event.key.toLowerCase();
  if (["w", "s", "arrowup", "arrowdown", " "].includes(key)) event.preventDefault();
  if (key === " " && !event.repeat) togglePlay();
  else keys.add(key);
});
window.addEventListener("keyup", event => keys.delete(event.key.toLowerCase()));
window.addEventListener("blur", () => keys.clear());
court.addEventListener("click", () => court.focus());

setMode("paused");
game.setAttribute("data-ready", "true");
requestAnimationFrame(frame);
